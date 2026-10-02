# 193 — An external agent's state is rewritten whole for everything it hears

**Status:** done
**Area:** Persistence

## The problem

`ExternalAgentActor::hear` stores the whole `ExternalAgentChannelState` for every
`FullResponseGenerated` in its channel. Since commit `7e22f5b` (issue
[179](179-external-agents-via-a2a.md)) that state keeps everything heard - tool results and a plan's
inner steps too - and a transcript holding a second copy of every message given, where a single id
was kept before. This was deliberate ("for the record").

## Why it matters

Each write is as large as the state, so the writes over a channel's life grow with the square of
its length; tool results can be large.

## Considerations

- Keep only what may be given or is needed for the record.
- A transcript of ids into the history instead of copies.
- An append operation on the `StateStore` - needed by any long-lived state, and natural for the
  Postgres store of issue [04](04-postgres-state-store.md).

## Done when

- Hearing a message costs a write independent of the length of the channel's history.
- The same holds for every state the `StateStore` keeps: a step of an agent's turn, a message
  appended to a channel's history, a change to an agent's memory (widened on 2026-10-02, see
  History).

## Plan: storing changes instead of states (option A)

Decided on 2026-10-02 (see History): the model records what changed in a state since it was last
stored, the `StateStore` stores that, and the JSON store keeps a snapshot and a journal per state.
What follows is the detailed plan; the steps at the end are separate commits.

*Revised by issue [197](197-one-chain-one-model-of-change.md), after steps 1 to 3 were done:*
nothing changes an entry of a chain after it was added, so there is no `Amended`, no `amend`, and
no `ChainChange`. A chain tells a store `Stored<E>` - the nodes new or changed in their place, and
the head - which is also its stored form and what `Chain::apply` merges; the state enums carry
`Stored<E>` where sections 1 and 3 say `Vec<ChainChange<E>>`, and the Postgres mapping of section
4 is an upsert of nodes and an update of the head. The enums are externally tagged, not with a
`part` tag: a variant such as `Turn(Option<TurnState>)` cannot be tagged internally. Sections 1
to 4 stay as the record of what was decided then; STORAGE.md says what is.

### What grows, and how it changes

Every state the `StateStore` keeps is a few small fields and some chains:

| State | Writer | Grows without bound | Small, rewritten whole |
|---|---|---|---|
| `AgentChannelState` | `AgentActor` | `history`, `transcript`, `notes`, `private_notes`, `prompts`, `tool_sets` | `processing_state`, `turn` |
| `ExternalAgentChannelState` | `ExternalAgentActor` | `history`, `transcript` | `offered_up_to`, `conversation`, `open_tasks` |
| `ChannelState` | `ChannelLoop` | `history` | `channel_id` |
| `AgentState` | `AgentStateHandle` | `memory` | nothing |

Everything that grows is a `Chain<E>` (`history`, `transcript`, the ledgers `notes`,
`private_notes`, `memory`) or a map of things kept once by digest (`prompts`, `tool_sets`). A chain
changes in exactly three ways, and every writer goes through them:

1. **An entry is appended** (`push_with`): the common case, and the only one that happens for every
   message heard or produced.
2. **The head moves** (`move_head`): when a turn is given up (issue 145), and for branching
   (issue 08). `active_child` is derived from the moves and needs no change of its own.
3. **An entry is amended in what its digest does not cover**: `Transcript::mark_given_up` sets
   `given_up`, `MessageHistory::reclassify` and `fit_into` set a message's classification. Today
   these go through `Chain::entries_mut`, the one path that changes entries without the chain
   knowing.

The maps only ever gain a key (`entry().or_insert`). The small fields are overwritten in place by
their writers (`state.turn = …`, `open_tasks.retain(…)`, `prepare_resume`); they are bounded by one
turn or one exchange, not by the channel's life.

### 1. `Chain` records its changes

`Chain<E>` gets a journal of what changed since it was last stored, and becomes the one place
where this is known. The change type is the wire format of the journal and, later, what a row of a
Postgres table holds:

```rust
/// A change to a chain since it was last stored (issue 193).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ChainChange<E> {
    /// An entry appended after the one `parent` names, which became the head.
    Added { parent: Option<Digest>, #[serde(flatten)] entry: E },
    /// The head moved to the entry `to` names - `None`: the root.
    Head { to: Option<Digest> },
    /// An entry changed in what its digest does not cover, as it is now; its
    /// digest says which one.
    Amended { #[serde(flatten)] entry: E },
}
```

Inside the chain, the journal is `pending: Vec<Pending>` with `Pending::{Added(usize),
Head(Option<Digest>), Amended(usize)}` - positions in `nodes`, so that nothing is cloned until the
changes are asked for. The field is `#[serde(skip)]`: a loaded chain has nothing pending, and the
stored form of a chain (`entries`, `head`) does not change.

What changes in the API:

- `push_with` records `Added` for a new entry, and only `Head` for one that is already there (the
  head moves to it, as today). `move_head` records `Head`.
- `amend(&mut self, digest: &Digest, change: impl FnOnce(&mut E)) -> bool` and
  `amend_each(&mut self, change: impl FnMut(&mut E) -> bool) -> usize` replace `entries_mut`;
  each records `Amended` for every entry the closure changed. `mark_given_up`, `reclassify` and
  `fit_into` are written over them and lose their own loops. There is no other way to change an
  entry any more.
- `changes(&self) -> Vec<ChainChange<E>>`: the journal since the last `mark_stored`, in order.
  Owned, which clones the new entries once per store: a borrowed form would need a second type
  per change (as `StoredRef`/`Stored` do for the whole chain) or generic associated types in
  `Journaled` below; the clone is proportional to what is new, which is the point.
- `mark_stored(&mut self)`: clears the journal.
- `apply(&mut self, change: ChainChange<E>) -> Result<(), String>`: replays one change - what a
  store does when loading a journal. It is **idempotent**: `Added` of a digest already there is
  the head move only, `Head` and `Amended` restate what is now. The JSON store relies on this
  (section 5). It fails the way `from_stored` fails: a parent that is not there, a head or an
  amended entry that is not there.

`entries_mut` is gone; `from_stored` is unchanged. Tests: every change is journaled once and in
order; `apply` of the journal onto a copy stored before gives the same chain (digests, entries,
branches, `leaf_from`); applying a journal twice changes nothing; `amend` leaves the digest alone.

### 2. What is kept once by digest: `Kept<V>`

`prompts: BTreeMap<Digest, String>` and `tool_sets: BTreeMap<Digest, Vec<ToolDefinition>>` grow
too - a new system prompt text per turn once notes or memory are in it - and a map has no order
to tell what is new. They become `Kept<V>`, a map by digest that is only ever added to and keeps
the digests added since it was last stored: `keep(digest, || value)` (the `or_insert` of today),
`get`, `len`, `iter`; `changes() -> Vec<(Digest, V)>`, `mark_stored`, `apply`. It serializes as
the map does today (a manual `Serialize`/`Deserialize` like `Chain`'s), so stored states are
unchanged. The alternative - writing both maps whole every time - keeps the write proportional to
the number of turns, which is what the issue is about; a type of its own is the smaller cost.

### 3. A state tells its changes: `Journaled`

One trait, in `caelumora_model::store` next to the `StateStore` it serves, makes the four states
one case for a store:

```rust
/// A state the `StateStore` keeps (issue 193): it tells what changed since it
/// was last stored, as a sequence of changes, so that a store writes only
/// that and replays such changes onto what it stored before.
pub trait Journaled: Serialize + DeserializeOwned + Send {
    /// One change to the state: a variant per part of it, carrying what that
    /// part records as a change.
    type Change: Serialize + DeserializeOwned + Send;
    /// What changed since `mark_stored`, in order.
    fn changes(&self) -> Vec<Self::Change>;
    fn mark_stored(&mut self);
    /// Replays `change` - idempotent, like `Chain::apply`.
    fn apply(&mut self, change: Self::Change) -> Result<(), String>;
}
```

A state's `Change` is an **enum with a variant per part of the state**, as the staged changes,
the ledger's `Change` and the events are built: a chain's part carries its `ChainChange<E>`, a
`Kept`'s one digest and value, a small field its whole value. Named `…StateChange`, with "state"
in the name on purpose: a change of the agent's or the channel's *state* - what was said, noted,
remembered - as against an update of the entity (`AgentUpdateEvent`, `update_channel`), which is
its name and composition:

```rust
/// A change to an `AgentChannelState` since it was last stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum AgentChannelStateChange {
    History(ChainChange<HistoryEntry>),
    Transcript(ChainChange<TranscriptEntry>),
    Notes(ChainChange<LedgerEntry<Notes>>),
    PrivateNotes(ChainChange<LedgerEntry<Notes>>),
    Prompt { digest: Digest, text: String },
    ToolSet { digest: Digest, tools: Vec<ToolDefinition> },
    ProcessingState(ActorState),
    /// The turn under way, whole: it is changed in place at every step
    /// (`Turn::current_mut`, `prepare_resume`), and is bounded by one turn.
    Turn(Option<TurnState>),
}
```

Likewise `ExternalAgentChannelStateChange` (`History`, `Transcript`, `OfferedUpTo`,
`Conversation`, `OpenTasks(Vec<OpenTask>)`), `ChannelStateChange` (`History`) and
`AgentStateChange` (`Memory`). The store handles a record with one `match`, and a Postgres store
maps every variant to one statement.

The rule is simple to state and to check: **the growing parts record what was added, the small
parts are recorded whole, at the end of every sequence**. `changes()` gathers each chain's and
`Kept`'s journal in a fixed order of the parts and appends the small parts; `apply` replays a
chain's change onto that chain and overwrites a small part. A sequence of nothing but the small
parts - `settle` after a step - is a small write, which is the goal. Order is kept within a part;
across parts it is the fixed order, which loses nothing: every reference between parts is a digest
or a message id, resolved only when read, never while replaying. `Transcript`, `MessageHistory`
and `Ledger<K>` delegate `changes`, `mark_stored` and `apply` to their chain, three one-line
methods each, so that the states need not reach into them. The four enums are data, not behaviour:
written by hand, no macro.

The journal records only what happened to a stored state. What a turn stages before it is taken
over is something else (section 7).

### 4. The `StateStore` API

The `put_…` operations keep their names and take the state **mutably**:

```rust
async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &mut AgentChannelState) -> StoreResult<()>;
```

The contract, in the trait's doc: a store stores the state, or only what changed since it was last
stored, as it chooses, and on `Ok` has called `mark_stored`; on `Err` nothing is marked, the
changes stay pending, and the next `put` includes them. That keeps the invariant "what is pending
is not stored" in the one place that knows, and the writers keep calling one method. The
alternative - `&T` and the caller marking stored on `Ok` - spreads the marking over four call sites
and is forgotten by the fifth.

The `get_…` operations are unchanged. `delete_channel_state` is unchanged in its contract.

- **`MemoryStore`**: `put` clones the state into its map and marks it stored. It never replays.
- **`JsonFileStore`**: section 5. It replays on load.
- **Postgres (issue 04)**: one `match` over the state's `Change`: a chain's `Added` is an insert,
  `Amended` an update of the entry's row, `Head` an update of the state's row, a small part a
  column of it; `get` reads the rows in order and builds the chain through `apply`, or
  `from_stored`. The shape is decided here, so that the two stores do not drift.

The store suite (`store_impl::suite`) gets: what was stored in two puts loads as one state with the
digests of the live one; a put after a failed put loses nothing (only testable on the JSON store,
with a journal made unwritable); `mark_stored` was called.

### 5. The JSON store: snapshot and journal

Per state, two files where there is one today:

- `<key>.json` - the **snapshot**, the state as `put` whole, as today. Every state file written so
  far is a valid snapshot without a journal, so **nothing has to be migrated**.
- `<key>.jsonl` - the **journal**: one line per `put`, the state's `Changes` as one JSON document.

`put(state)`:

1. If there is no snapshot, or the journal is at least as large as the snapshot and larger than
   a floor: write the snapshot through `write_json_atomic`, then truncate the journal. A write is
   therefore never larger than twice what was written since the last snapshot, and over a
   channel's life the bytes written are a small constant times the state's final size, instead
   of its square. The floor keeps small states from being compacted at every other write; it is
   64 KiB unless configured, as `journal_floor` beside `path` of `PersistenceBackend::Json`
   (`persistence.state_store`, CONFIGURATION.md). Both sizes are asked of the file system, two
   `metadata` calls per `put`; the store keeps nothing in memory between calls.
2. Otherwise: append one line to the journal (`OpenOptions::append`, one `write_all`). Should the
   write fail, the file is truncated back to its length before the write, so that a torn line is
   not followed by a good one; and the error is returned with the changes still pending.
3. `mark_stored`.

`get`: read the snapshot (or the default), then every line of the journal in order through
`apply`. A last line that does not parse is what a crash during an append left and is dropped
with a `warn!`; a line before the last that does not parse is a corrupt state and a `Backend`
error, as a corrupt file is today. A missing journal is an empty one.

What a crash leaves, and why it loads:

- **During an append**: a torn last line, dropped. The state is as of the previous `put`, which is
  what a torn whole-file write leaves today.
- **Between the snapshot and the truncation**: the journal still holds changes the snapshot
  already has. They are replayed and change nothing, because `apply` is idempotent. Lines appended
  afterwards follow them and are applied. The next `put` compacts again.
- **During the snapshot**: `write_json_atomic` leaves the old snapshot; the journal is intact.

Compaction needs no second copy of the state: `put` has the whole state in hand every time, which
is why the API hands over the state and not only its changes. `delete_channel_state` removes the
directory, both files with it.

The four `get`/`put` pairs of the store collapse into one generic pair, `load<T: Journaled>(path)`
and `store<T: Journaled>(path, &mut T)`, plus the paths; the `MemoryStore` likewise keeps one
`put_in` for its four maps.

### 6. Where the runtime changes

- `ActorRuntime::store_channel_state` and `store_external_channel_state` take `&mut`.
- `Turn` offers its state mutably for storing (`agent_channel_state_mut`, `pub(crate)`), used by
  `AgentActor::store_channel_state`; `catch_up` of both actors passes `&mut state`.
- `ChannelLoop::append_history`, `reclassify` and `reclassify_message` pass the guard mutably.
- `AgentStateHandle::apply` clones the current state, applies the turn's changes, puts the clone
  (which marks it stored) and swaps it in - the same sequence as today.
- `Turn::keep_tools` and `keep_prompt`, `ContextResolver` and the tests that fill `prompts` and
  `tool_sets` use `Kept`.
- Nothing in `caelumora-rest-server`: it only reads states through the views.

### 7. Notes and memory

Notes, private notes and memory are ledgers (issue 92): a chain of changes, only ever appended to,
so the ledger's design is exactly what a journal wants. A turn stages them in its `TurnState` -
part of the whole-written turn while it runs - and `commit_turn` pushes them into their chains,
which journals them; `AgentStateHandle` stores the memory's chain the same way as section 6 says. A
branch move of a ledger (issue 08) is a `Head` like any other.

What is staged and what is journaled are two values and stay two types. A staged note is a
`NoteChange::Add { id, text, classification }`: an intent. Its journal entry is the `LedgerEntry`
the commit makes of it, with `origin`, `after`, `written_at`, `digest` and `parent` - none of
which exists before `commit_turn`. One type for both would carry those as options empty until the
commit.

The two enums that stage today, `AgentChannelChange` (`Note`, `PrivateNote`) and
`AgentStateChange` (`Memory`, its one variant), do nothing but bundle ledgers and dispatch to them
in `apply`. They are dissolved, which also frees the name `AgentStateChange` for the journal:

- `TurnState.channel_changes` becomes `notes: Vec<NoteChange>` and `private_notes:
  Vec<NoteChange>`; `record_turn` pushes each list into its ledger with `NoteOrigin { transcript
  }` and `after`, as `AgentChannelState::apply` does today, which goes. `Turn::stage_channel_change`
  and `staged_channel_changes` become `stage_note(kind, change)` and `staged_notes(kind)`;
  `NoteKind::change` and `NoteKind::of` in `agent_notes.rs`, which only wrap and unwrap the enum,
  go.
- `TurnState.agent_changes` becomes `memory: Vec<(MemoryChange, MemoryOrigin)>`;
  `AgentStateHandle::apply` takes that list, `AgentState::apply` goes.

What this gives up, said plainly: STORAGE.md and MEMORY.md describe `Turn` and the handle as
generic over the kinds of agent state (issue 62), knowing nothing of what the state holds. After
this they know notes and memory by name - as `Turn::saw_private_knowledge` already does. With one
kind per state the generic layer is provision for what has not come; a third kind is a field in
`TurnState` and a push in `commit_turn`, no harder than a variant. Both documents are updated.

### 8. Other operations considered

- **Branching (issue 08)**: `move_head` on history, transcript and ledgers is `Head`. `active_child`
  is set by replaying the moves as `move_head` sets it live, so a loaded chain switches branches
  as the live one would.
- **Reclassification (issue 133)**: `reclassify` and `fit_into` on the channel's history are
  `Amended`; `fit_into` after lowering a channel amends every message that changed in one
  record - rare, and bounded by one history. The agents' own histories are not reclassified today
  (nothing takes `MessageReclassified` into an `AgentChannelState`); whether they should be is a
  question of issue 133, not of this one.
- **A turn's working state**: `TurnState` is written whole after every step, with its entries, its
  tools and its tool calls - bounded by the turn, not the channel, which the "done when" allows.
  A long turn of many tool steps pays for its own length; section 9 says what would end that.
- **Resuming** (`prepare_resume`) changes the turn in place: the small part, whole in the next
  record.
- **Deleting**: unchanged. **Retention** (issue 16): a snapshot that drops old entries is a
  snapshot like any other; the journal does not stand in its way.
- **Several instances (issue 122)**: the number of records since the snapshot is a natural version
  for the store to refuse an outdated write by; not built here, not precluded.
- **Reading for the views**: `get` costs the snapshot plus the journal, at most twice the state;
  the views read a state per request today and go on doing so.

### 9. What gets simpler, and what could

- `Chain::entries_mut` goes: the amendments are named operations of the chain, and `mark_given_up`,
  `reclassify` and `fit_into` lose their hand-written searches.
- The JSON store and the memory store each keep one implementation for four kinds of state.
- `MessageHistory` and `Transcript` stay two types over one `Chain`: they differ in their entry and
  their amendment, and in what they mean (the channel's record, what the model was given). They
  are not unified here.
- **The turn as a branch** would be the real simplification of `Turn`: the messages a turn produces
  could go into the transcript as they are produced - the chain already keeps branches - with the
  turn remembering only the digest it began at; taking the turn over would be nothing, giving it
  up a `Head` back and a mark, and the `TurnEntry` list, the `expected` digest check in
  `record_turn` and the whole-turn write per step would disappear. It touches what issues 136 and
  145 fixed (the transcript does not change while a turn runs; `TranscriptView` continues the
  stored transcript with the turn's messages) and what the history records when. That is an
  architecture decision of its own, to be weighed after this issue, not bundled into it.

### 10. Steps

1. **Model: the staged changes lose their enums** (section 7). `TurnState` stages notes, private
   notes and memory in fields of their own; `AgentChannelChange` and `AgentStateChange` go, with
   the `apply` methods that dispatched on them; STORAGE.md and MEMORY.md follow. No behaviour
   change; the stored form of a turn changes, which only a turn under way at the upgrade would
   notice, and none is (issue 108).
2. **Model: the chain journals its changes.** `ChainChange`, `amend`/`amend_each`, `changes`,
   `mark_stored`, `apply`; the wrappers delegate; `entries_mut` removed. No behaviour change, the
   stored form unchanged.
3. **Model: `Kept<V>` and `Journaled`.** The four states and their `…StateChange` enums;
   `prompts` and `tool_sets` become `Kept`. Tests: changes replayed onto a copy give the same
   state.
4. **Store API: `put_…` takes `&mut` and marks stored.** Memory store, runtime call sites, suite.
   The JSON store still writes the state whole.
5. **JSON store: snapshot and journal.** Append, load with replay, compaction, the torn last line,
   the crash cases above as tests.
6. **Docs and issues.** STORAGE.md gets a section on storing changes (the `Journaled` contract,
   the two files, compaction); AGENT-TURN.md's "Storing and resuming" says what a step writes;
   issue 04 records the row shape its store inherits.

The cost is real: steps 1 to 3 touch `chain`, `transcript`, `message_history`, `ledger`, `agent`,
`external_agent`, `channel` and `context` in the model and the turn's staging in the runtime; step
5 is a new piece of the JSON store with its own failure modes. Nothing of it is throw-away: the
Postgres store of issue 04 is built on the same `Journaled` contract and the same change types.

### Decisions

Decided on 2026-10-02: a state's changes are an enum with a variant per part, not a struct of
lists; the enums keep "state" in their names; the staged changes lose their enums; `put_…` keeps
its name and takes `&mut`; the trait is `Journaled`; the compaction floor is 64 KiB and
configurable; the sizes come from the file system. Whether the turn becomes a branch (section 9)
is left open until a step needs the answer - then it is done at once or opened as an issue.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series; a trade-off against the
  decision of `7e22f5b`, to be weighed by the maintainer.
- **2026-10-02** — Clarified with the maintainer: "a second copy of every message given" means that
  a given message is stored twice in the same document - in the history and in the transcript -
  not that it is given twice. The transcript grows by exactly the messages given in each exchange,
  once each, as intended (`exchange`, `unheard`); it is the record of what was given, and keeps
  whole messages as an agent's own transcript does. What remains is the cost: the whole state is
  written for every message heard, and the history keeps what is never given.
- **2026-10-02** — Widened to every state the `StateStore` keeps: since issue 196 an `AgentActor`
  writes its whole `AgentChannelState` for everything it perceives and after every step of a turn,
  a `ChannelLoop` its whole history for every message, and the cost grows the same way. Options
  weighed with the maintainer: (A) the model records its changes and the store appends them, with
  a snapshot and a journal in the JSON store; (B) the store compares what it wrote last and
  appends the difference - rejected, it hides knowledge of the states' shape in the store and the
  Postgres store of issue 04 could not reuse it; (C) a transcript of ids into the history - halves
  the size, still quadratic; (D) leave it to issue 04. Decided for A; the plan above is the
  result, to be carried out in the steps it names.
- **2026-10-02** — Plan revised with the maintainer: a state's changes are one enum with a variant
  per part, in the pattern of the ledger's `Change` and the events, so that a store handles a
  record with one `match` and keeps the order; "state" stays in the names, since a change of the
  state is not an update of the entity; the staged `AgentChannelChange` and `AgentStateChange`,
  which only bundled ledgers, are dissolved into fields of `TurnState`, as a first step of its own.
- **2026-10-02** — Step 1 done: `TurnState` stages notes, private notes and memory in fields of
  their own (`notes`, `private_notes`, `memory`); `AgentChannelChange`, `AgentStateChange` and the
  `apply` methods that dispatched on them are gone, `Turn::stage_note`/`staged_notes` and
  `stage_memory`/`staged_memory` replace the generic staging, `AgentStateHandle::apply` takes the
  memory changes. STORAGE.md and MEMORY.md follow.
- **2026-10-02** — Step 2 done: `Chain` keeps a journal of what changed since it was last stored
  (`ChainChange`: `Added`, `Head`, `Amended`; `changes`, `mark_stored`, `apply`), and `amend` and
  `amend_each` replace `entries_mut` as the only way to change an entry. `Transcript`,
  `MessageHistory` and `Ledger` delegate. The stored form of a chain is unchanged.
- **2026-10-02** — Step 3 done: `Kept<V>` keeps the prompt texts and tool sets by digest and
  records what was added; `Journaled` (in `store`) with `AgentChannelStateChange`,
  `ExternalAgentChannelStateChange`, `ChannelStateChange` and `AgentStateChange`, each a variant
  per part of its state. The stores still write whole states; nothing stored changes its form.
- **2026-10-02** — Paused after step 3. Reviewing steps 2 and 3 showed two models of change
  over one chain and a type for every step between them; that is
  [197](197-one-chain-one-model-of-change.md), to be done first. Steps 4 to 6 go on from there
  with a journal that is the chain's stored form and has no `Amended`.
- **2026-10-02** — Step 4 done (after issue 197): every `put_…` of the `StateStore` takes the
  state mutably and marks it stored on `Ok`; the `MemoryStore` marks before it keeps its copy, the
  `JsonFileStore` after writing the whole state, as it still does. The runtime passes its states
  mutably (`Turn::agent_channel_state_mut`). The suite checks that what was put has nothing
  pending beyond what a loaded state has.
- **2026-10-02** — Step 5 done: the `JsonFileStore` keeps a snapshot and a journal per state,
  appends a put's changes as one line, and folds the journal into a new snapshot once it is as
  large as the snapshot and `journal_floor` (64 KiB unless configured as
  `persistence.*.journal_floor`). A torn last line is dropped, a failed append cuts the journal
  back, a journal that survived its snapshot replays without effect; a corrupt line before the
  last is a corrupt state. Tested for each. Hearing a message now costs the message's line,
  independent of the history's length.
- **2026-10-02** — Step 6 done, and the issue with it: AGENT-TURN.md says what a step writes,
  STORAGE.md what a state tells a store and how the JSON store keeps it, issue 04 what its store
  inherits. Done when, checked: a message heard, a step of a turn, a message appended to a
  channel's history and a change to an agent's memory each cost one line of the state's journal,
  independent of the length of what came before; a snapshot is written only once the journal has
  grown to its size, so the bytes written over a state's life are a small multiple of its final
  size.
