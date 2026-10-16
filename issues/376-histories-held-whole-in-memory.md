# 376 — Histories and transcripts are held in memory whole

**Status:** in progress
**Area:** Persistence

## The problem

A channel's history (`ChannelState::history`) and an agent's history and transcript
(`AgentChannelState::history`, `::transcript`) are each a `Chain` whose nodes all sit in a `Vec`
in memory, from the first entry of the channel to its last. They are loaded whole from the
`StateStore` - snapshot and journal - when a channel or an agent's actor starts, and kept whole as
long as it runs. Since [193](193-external-state-rewritten-whole.md) a change costs one line of the
journal, independent of what came before; what is held and what is read has no such bound.

Almost everything that works on them uses only their recent end: a turn's context is the
transcript's active path, and compaction already shortens what the model is given, not what is
held; the web UI shows the last messages first ([218](218-paged-histories.md)); catching up takes
what an actor missed since it last listened (issues 190, 196,
[223](223-full-inbox-drops-events.md)). The older part is kept for the record and read rarely - "show earlier", an audit, a branch switched
back to.

## Why it matters

- Memory grows with every channel's length, times every agent taking part, for as long as the
  server runs - and a long-running channel is what the agents are meant for.
- Starting a channel or an actor reads its whole past, however little of it is used.
- [218](218-paged-histories.md) can page what is sent, but not what is read: it notes that reading
  only part of a history is a matter of the stores. Without it, paging saves transfer and rendering
  only.
- [223](223-full-inbox-drops-events.md) is to repair what a full inbox dropped by catching up from
  the channel's history. Built on the history as it is, that would read it whole and take the
  assumption that it lies in memory deeper into the code.

## Considerations

- **Stable positions first.** Reading a part needs a way to name where it begins that does not
  change as the chain grows or is held in part: a sequence number per entry, in the order entries
  were added, rather than an index into `nodes`. The same numbers serve 223 (an actor knows up to
  which entry of the channel it has heard, and notices a gap) and 218 (pages before a position,
  where a message id is ambiguous on a chain with branches).
- **A range API before spilling.** `History` and `Transcript` would offer reads by range - the
  entries after a position, a page before one - and their callers use those instead of the whole
  chain. As a refactoring without a change of behaviour, it lets 218 and 223 be built on it while
  the chain is still whole, and spilling then changes only what is behind it.
- **What stays in memory.** A window at the head: the active path's recent part, what the next turn
  needs. Older nodes are dropped once stored, and read back from the store on request.
- **The store.** Snapshot and journal (`JsonFileStore`) are read whole; reading a range needs a
  stored form addressable by position - entries per row in a database
  ([04](04-postgres-state-store.md)), segments in the JSON store. Whether the `StateStore` gains
  range reads or the chains get a store of their own is open.
- **What reads across the whole chain today** and would have to work from a window:
  - `messages()` resolves each message as it stands from every later `Modified`, `Revoked`,
    `Reclassified` and `Fitted` entry; a late change to an old message must still be found, and a
    `Fitted` applies to everything before it.
  - `missing_from` collects the ids of every message held to find what an actor lacks; with
    sequence numbers it becomes "the entries after mine".
  - The active path and the branches: a window over a tree is not a suffix of a list. Switching to
    an old branch, or a retry far back, needs nodes that were dropped.
  - The digest chain itself survives a window: each node carries the digest up to it, so a window
    may begin mid-chain.
- **The transcript is not the history.** It changes in ways the history does not - `start_anew`,
  compaction, its active path is what the model is given - so what can be dropped differs: in a
  transcript mainly what a compaction replaced and what was given before a `start_anew`.

## Done when

- A channel's history and an agent's history and transcript keep in memory a bounded part of
  themselves, independent of the channel's length, and load the rest from the store on request.
- Starting a channel or an actor does not read its whole past.
- 218 and 223 read by range, through the same API.
- STORAGE.md and AGENT-TURN.md say what is held and what is loaded.

## Plan: positions, ranges and sums, without a change of behaviour

Decided on 2026-10-16 (see History): this issue is the first step of the epic
[378](378-resource-management-epic.md). It changes nothing in what a chain says or what a state
holds for its holders; it changes how both are reached, so that
[379](379-history-stored-as-it-stands.md) and [380](380-transcript-window.md) can put a store
behind the chains and 218 and 223 can read by position. Everything here is `caelumora-model`, with
a handful of callers in the runtime and the agents following. The steps at the end are separate
commits.

### 1. Every entry has a position

`Node<E>` gets `seq: u64`, the position of the entry in the order entries were added - the root of
the first branch at `0`, every entry after it one more, whichever branch it is on. The position is
assigned by the chain when the entry is added (`Chain::add`), stored with the node like `digest`
and `parent`, and never changed: nothing is ever removed, so it is stable for the chain's life. It
is not covered by any digest, since it follows from the order and says nothing about content.

```rust
pub struct Node<E> {
    /// Where the entry is in the order entries were added (issue 376): the
    /// address of the entry in a store, stable for the chain's life.
    pub seq: u64,
    pub digest: Digest,
    pub parent: Option<Digest>,
    pub(crate) active_child: Option<Digest>,
    pub entry: E,
}
```

`Node::new`, which nothing outside the chain uses any more, takes the position and becomes private
to the chain: a node is made where it is added. `Chain` keeps its `nodes` in order, so
`nodes[i].seq == base + i` is an invariant (`base` is `0` until step 6), and `index` maps a digest
to a position as before.

`Stored<E>` - the stored form, and what a chain tells a store since it was last stored - carries
the nodes with their positions. `Chain::apply` checks them as it checks the parents: a node that is
new must have the position after the last (`seq == self.end()`), a node that is there already the
position it has; a node out of place does not fit, and nothing changes. This is what lets a store
hand out nodes by range and a chain know where a range belongs. The JSON store's journal lines and
snapshots change with it; **every stored state (`state/` of the JSON store) is to be deleted**, as
the rules allow.

The ledgers (`Ledger<K>`) are chains and get positions with everything else; nothing of theirs
changes besides.

### 2. The chain reads by range

Four methods on `Chain<E>`, all over the order entries were added, all branches, as `all_entries`
is - a range is a range of the stored chain, not of the active path:

- `end(&self) -> u64`: the position after the last entry; what the next entry gets, and what an
  actor remembers it has heard up to (223).
- `at(&self, seq: u64) -> Option<&Node<E>>`: the entry at a position.
- `from(&self, seq: u64) -> impl DoubleEndedIterator<Item = &Node<E>> + ExactSizeIterator`: the
  entries from a position on, in order; empty from `end()` on. "The entries after mine" of 223 is
  `from(heard_up_to)`.
- `before(&self, seq: u64, limit: usize) -> impl DoubleEndedIterator<Item = &Node<E>>`: the last
  `limit` entries before a position, in order; what a page of 218 is cut from while the chain is
  whole, as `before(end(), limit)` for the first page and `before(oldest of the page, limit)` for
  the next.

`seq_of(&self, digest: &Digest) -> Option<u64>` names the position of a digest, for whoever holds a
digest and wants a range from it. `all_entries` stays, as `from(base)`; `entries` and `prefix`
stay the active path and a path to a digest.

### 3. The history folds any range the same way

`History::fold_nodes` is today a loop over `all_entries()` with the rule of every change in its
match arms. It becomes two things:

- **The rule, on the entry:** `HistoryEntry::fold_into(&self, message: &mut Message) -> bool`
  applies what the entry changes of `message` as it stands - `Modified`, `Revoked`, `Reclassified`
  if they name its id, `Fitted` always - and says whether it did; every other entry changes nothing.
  This is the rule 379 applies when a store writes an entry into the derived store and when it
  rebuilds it, so it lives on the entry and nowhere else.
- **The fold, a function over nodes:** `history::fold<'a>(nodes: impl IntoIterator<Item = &'a
  Node<HistoryEntry>>) -> Vec<(u64, Message)>`: the messages the nodes say, each with the position
  of the node that said it, with every later node of the range folded into it. Given
  `all_entries()` it is what `fold_nodes` is today; given a range, it is the range as it stands
  with respect to the changes in the range - which is complete for every message the range begins
  before, since a change always comes after what it changes.

The readers are written over them, each over no more than it needs:

- `messages()` and `items()`: `fold(self.all_entries())`, cut to the active path as today. The
  history is linear in every production use (`move_head` on a `History` occurs in tests alone),
  so this is the whole chain, and 379 pages it; nothing to narrow here.
- `message(id)`: the position of the `Said` entry of `id`, found from the end backwards, then
  `fold(self.from(at))` and the message of that position - not a fold of everything. The three
  checks before a change (`reclassify`, `modify`, `revoke`) use it as they do.
- `said_in_turn(actor, turn)` takes the actor whose turn it is - both callers, `Turn::messages`
  and the turn's ending, know it - and reads from the end backwards to the end of the actor's
  previous turn: a `TurnFinished` or `TurnGivenUp` of `actor` with another turn id, or the start.
  An actor has one turn at a time and a turn given up is answered again under a new turn id
  (issue 259), so every message of `turn` lies after that end, and every change to one of them
  after the message. What it collects is `fold` of that tail, filtered as today: the messages
  produced in `turn`, none taken back.
- `fit_into(channel)` records `Fitted` and returns nothing: the count of messages it changes was
  the only reason to fold everything, and its one caller (`ChannelLoop`, `settings.rs`) stores the
  state whenever the entry is recorded, which it should - today an entry that changed nothing is
  stored only with the next put.
- `messages_after(last)` has no caller and is removed.
- `missing_from`, `last_message_id`, `turn_given_up_last`, `extend`, `push` and the `record_…`
  methods are unchanged; `missing_from` is 223's to rewrite over positions.

`Transcript` needs none of this: `as_they_stand` folds a path it is given, which is already the
function 380 needs for a loaded branch; `message(id)` over `all_entries` stays until 380 moves the
part below the window to the store.

### 4. Sums over whole chains become small parts of the state

Two values are summed over every entry of the transcript today and would read what 380 no longer
holds. Both become fields that are kept as the entry is added - recorded, like the other small
parts, whole in `changes()` - and read from there:

- **`AgentChannelState::usage: Option<TokenUsage>`**, the total of every call the agent's model
  reported in this channel: `calls`, `prompt_tokens`, `completion_tokens` added up, `context_size`
  that of the last message that reported one, as `TokenUsage::of` builds it. Added to in
  `AgentChannelState::said` when the message said carries tokens - the one place a message of the
  agent's own enters the transcript for the first time; the copies a compaction makes
  (`push_entry`) do not pass it, so a message is counted once, as today. `usage(agent)` returns the
  field with `context_tokens` from `next_context_tokens`, as today, and loses its argument: the
  field is this agent's. `None` stays "unknown": an external agent's calls report no tokens. A new
  variant `AgentChannelStateChange::Usage(Option<TokenUsage>)`.
- **`RemoteState::seen: Option<Classification>`**, the join of every classification the remote
  side was ever given - every message and every change of one on every branch (issue 236). Joined
  in `ExternalAgentActor` where it is summed today, before the call, over the active branch alone:
  `state.remote.see(classification)` for each entry of `transcript.entries()` with one. Every entry
  of an external agent's transcript is on the active branch at the time of exactly one call - the
  branch starts anew after each - so the join over the branches at the calls is the join over all
  entries. `work.see(&seen)` then. `RemoteState` is recorded whole already (`Remote(RemoteState)`),
  so nothing changes in `changes()`; `is_empty` takes the field into account.

### 5. The resolver reads one branch of the history

`ContextResolver::token_positions` builds `produced_after` - which transcript entry each message of
the agent's was produced after - from every `Said` entry of the agent's history. It uses it only
for messages on the path it resolves that carry `prompt_tokens`, which a copy made by a compaction
never matches ("which a copy kept through a compaction does not"): only messages produced on the
current branch, all said after the `Compacted` entry that began it. `History` offers

```rust
/// What the agent `agent` produced on the current branch of its transcript, each message with
/// the transcript entry it was produced after: the `Said` entries with a context from the end
/// back to the latest `Compacted` entry, which began the branch (issue 16).
pub fn produced_on_branch(&self, agent: ActorId) -> impl Iterator<Item = (MessageId, Option<Digest>)> + '_
```

and the resolver builds its map from that. A history without a compaction is read whole, which is
then the size of the transcript's one branch. `recorded_context_of(id)`,
`AgentContextView::produced` and `::saw` keep reading backwards over everything for an old
message: that part is 379's and 380's, since it is what the stores answer. `next_context_tokens` finds the profile of the agent's
last message from the end backwards already.

### 6. The chain may hold a suffix

The hinge to 379 and 380, in `Chain` alone, with nothing trimming yet: a chain may hold the
entries from a position `base` on instead of from `0`, and know that what lies before is older and
a store's.

- `base: u64`, `0` for a chain built here; `nodes[i].seq == base + i`; `end() == base +
  nodes.len()`; `at`, `from` and `before` answer nothing for positions below `base`.
- `Chain::suffix(base: u64, stored: Stored<E>) -> Result<Self, String>` builds a chain from the
  nodes of a range that begins at `base`: the first node may follow an entry that is not there,
  which is older and trusted; every other node is checked as `apply` checks. `from_stored` is
  `suffix(0, …)`.
- `path_to` and `path_from` walk parents until the root or until a parent that is not in the chain,
  which is older: the active path a suffix knows is the part of it from `base` on. `entries` and
  `len` say so in their docs; the transcript's active branch always lies within its window (380),
  the history's active path is the whole chain, of which the window is the end.
- `apply` with a parent below `base` is as for `suffix`: accepted as older. `push_with` finds a
  digest in the window only; a repeated digest below the window becomes a second node. A digest
  covers the message's id (`digest::message_chain`), so this needs the same message pushed again
  after the same predecessors, which nothing does; the stores must not make digests unique all the
  same.
- `move_head` to a digest below `base` returns `false`, as to one that is not there: whoever needs
  it loads the range first (380).

Unit tests of the chain cover a suffix built from the end of a chain: the same `digest`, `end`,
`from` and `before` as the whole chain for everything from `base` on, `entries` the path from
`base`, `apply` of what the whole chain changed since, and `prefix` of a digest below `base` as
`None`.

### What is documented

- STORAGE.md, *what a state tells a store*: a chain's nodes carry their position; a store keeps
  them addressable by it (what 379 and 380 build on), and `apply` places them by it.
- AGENT-TURN.md, *the transcript as a chain*: an entry's position beside its digest - the digest
  names the content up to it, the position where it is; *token usage*: read from the state, counted
  as a message is said.
- The doc comments of `Chain`, `Node`, `History`, `fold_into`, `fold`, `said_in_turn` and
  `produced_on_branch`.

### Steps

1. **Positions.** `seq` in `Node`, set by `add`, checked by `apply`; `end`, `at`, `from`,
   `before`, `seq_of`. Tests: positions are consecutive across branches, survive a round trip
   through `Stored` and the JSON store (store suite: a state stored in two puts loads with the
   same positions), and a node out of place is not applied. Stored states are deleted.
2. **The fold.** `HistoryEntry::fold_into`, `history::fold`; `messages`, `items`, `message`,
   `said_in_turn(actor, turn)` over them; `fit_into` returns nothing and its caller stores always;
   `messages_after` removed. Tests: `fold` of a range equals the same messages of `fold` of the
   whole chain; `said_in_turn` finds a turn's messages past another actor's turn end and stops at
   the actor's own previous one.
3. **Sums.** `usage` as a field added to in `said`, `seen` on `RemoteState` joined before a call.
   Tests: `usage` after two messages with tokens and a compaction equals today's sum; `seen`
   covers an entry of an earlier call (the test at `external_agent.rs` that checks what a lowered
   channel refuses).
4. **The resolver.** `produced_on_branch`; `token_positions` over it. Tests: the positions of a
   branch after a compaction are those of today (the summary compaction tests cover them).
5. **Documents.** STORAGE.md, AGENT-TURN.md, this issue's History.
6. **A suffix.** `base`, `suffix`, the walks that stop at `base`, `move_head` below it. Tests as
   above. Nothing calls `suffix` outside the tests until 380.

Steps 1 and 6 touch `Chain` and are to be reviewed together with its invariants; 2 to 4 are
independent of each other and of 6.

## History

- **2026-10-15** — Issue created while weighing options for
  [223](223-full-inbox-drops-events.md): catching up from the channel's history only moves the
  problem while that history is held whole, and [218](218-paged-histories.md) pages what is sent
  but not what is read. Proposed order: stable positions and a range API as a refactoring first,
  218 and 223 on it, spilling behind it.
- **2026-10-16** — Options weighed with the maintainer; the frame decided is recorded on the epic
  [378](378-resource-management-epic.md). This issue is narrowed to the refactoring that prepares
  the rest, without a change of behaviour: a stable position (`seq`) per entry, reads by range on
  `Chain`, the sums over whole chains (`usage`, the classification an external agent's remote side
  has seen) turned into small parts of the state, and every reader that needs only the end or one
  entry taken off `all_entries()`. The history's half - a derived store of the items as they stand,
  read in pages - is [379](379-history-stored-as-it-stands.md); the transcript's half - the active
  branch in memory, older branches in the store - is [380](380-transcript-window.md). The *Done
  when* above is reached by the three together.
- **2026-10-16** — The plan above written at the maintainer's request: what the narrowed issue
  builds, and in which steps.
- **2026-10-16** — Step 1 done: every node carries its position (`Node::seq`), given when it is
  added and checked when a stored chain is merged; `Chain` reads by range (`end`, `at`, `from`,
  `before`, `seq_of`). Every stored state is to be deleted.
