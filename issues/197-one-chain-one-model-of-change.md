# 197 — Two models of change over one chain, and a type for every step between them

**Status:** done
**Area:** Code structure

## The problem

Everything that grows in a state is a `Chain<E>` (issue 48): a tree of digested entries with a
head. Around it stand more types than the one idea needs, with no clear line between them:

| Type | What it is |
|---|---|
| `Chain<E>`, `Node<E>` | the container, and an entry at its place in it |
| `ChainChange<E>` | what the container did since it was stored: `Added`, `Head`, `Amended` (issue 193) |
| `Ledger<K>`, `Transcript`, `MessageHistory` | wrappers over one chain each: content-specific methods, the rest delegation |
| `LedgerEntry<K>`, `Change<Id>`, `Item<K>` | a ledger's entry, what it says, and what stands after folding |
| `TranscriptEntry`, `HistoryEntry` | the message chains' entries, each with a part changed in place |

Behind it are two models of "something changes", in one codebase:

- The ledger (issue 92) appends: reclassifying a note is a `Change::Reclassify` added to the chain,
  covered by the digest, and the state is the fold over the chain. Nothing is ever changed.
- The message chains amend in place: reclassifying a message overwrites `message.classification`
  (issue 133), giving up a turn writes `given_up` on an entry (issue 145), lowering a channel
  rewrites every message's classification (`fit_into`) - all in the part a digest does not cover.

The second model is why `entries_mut` existed, why `amend` and `amend_each` replaced it, and why
the chain's journal is an enum with an `Amended` variant. Without in-place changes, what a chain
has to tell a store is "the new entries and the head" - exactly its stored form, `Stored { entries,
head }`, so that snapshot and journal would be one type and `apply` a merge. And the wrappers add
nothing but names: every method of theirs that is not content-specific forwards to the chain.

The names show the strain: `Ledger::changes` was the chain of `Change`s before `changes()` became
the journal; `ChainChange` beside `Change`; `AgentChannelChange` had to go so that
`AgentChannelStateChange` could come. A new name for every step is the sign that the steps are
not distinct things.

## Why it matters

A reviewer has to hold six types in mind to read one append, and two models to know what a digest
covers. Every further feature on a chain - branching (issue 08), retention (issue 16), the
Postgres store (issue 04) - is built twice or decides again which model it follows.

## Considerations

- **One container, which only appends and moves its head.** `Chain<E>` keeps `Node`, `push_with`,
  `move_head`, `prefix`, the branch bookkeeping. Its journal is its stored form: what is told to a
  store is `Stored<E>` - the entries added since, with the head - and `apply` merges one into the
  chain. `ChainChange`, `amend` and `amend_each` go.
- **Entries are events; the state is the fold.** Reclassifying a message becomes an entry of the
  history, `HistoryEntry::Reclassified { message, classification }`; lowering a channel one entry,
  `Fitted { channel }`, instead of a change to every message; `messages()` folds. Giving a turn up
  becomes an entry of the transcript after the head went back, `TranscriptEntry::GivenUp { end,
  reason, at, turn_entries }`; the model is given the messages only. The record then shows what
  the classification was and when it changed, which is what an audit log wants (issue 140).
- **No wrappers.** `Transcript`, `MessageHistory` and `NoteChain` become aliases of `Chain<…>`
  with inherent impls for the content: `push` with the content's digest scheme, `messages`,
  `given_up_turns`, `current`, `next_id`, `with_staged`. The generic primitives - `push_with`,
  `amend` while it exists - become `pub(crate)`, so that a transcript still cannot be extended
  with anything but a message. The stored form loses one level of nesting (`{"messages": {…}}`),
  which no document has to survive (issue 108).
- **One change type per content, one model for all.** `Change<Id>` stays the ledger's; a message
  has no `Remove` and carries its classification itself, so one type for every content would be
  forced. What is unified is the model: every content is a sequence of events in one container.
- **What the digest means.** A transcript's digest then changes when a turn is given up, though the
  model saw nothing new: the digest names a prefix of the record, not only what the model was
  given. AGENT-TURN.md has to say so, and `ContextRef`s resolve as before.
- **What it touches.** `ContextResolver`, `TranscriptView`, `recorded_context_of`,
  `missing_from`, `given_up_turns`, the views, `channel.rs` reclassifying, the digest schemes
  (`crate::digest`) for the new variants, and their tests. Issue 193 pauses after its step 3 and
  goes on from here with a journal that has no `Amended`.

## Done when

- `caelumora_model::chain` has one container and one stored form, which is also what a store
  appends; nothing changes an entry after it was added.
- Each content is an entry type with inherent methods on its chain; no type exists only to forward
  to another.
- The record of a channel and of an agent shows a reclassification and a turn given up as entries,
  with the digest covering them.

## Plan

### The types afterwards

One container, in `chain.rs`, which owns everything that is about the structure - the digest, the
predecessor, the branch bookkeeping - so that a content type is nothing but its payload:

```rust
/// An entry at its place in the chain.
pub struct Node<E> {
    pub digest: Digest,
    pub parent: Option<Digest>,
    pub(crate) active_child: Option<Digest>,
    pub entry: E,
}

pub struct Chain<E> { nodes: Vec<Node<E>>, head, index, path, pending: BTreeSet<usize> }

/// A chain as it is stored: nodes, and the head. Also what a chain tells a
/// store since it was last stored - the nodes that are new or whose place
/// changed, and the head - and what `apply` merges into it.
pub struct Stored<E> { pub entries: Vec<Node<E>>, pub head: Option<Digest> }
```

`ChainEntry` goes: the digest is the node's, computed by the content's scheme when it is pushed
(`push_with(|previous| (entry, digest))`, `pub(crate)`). `amend`, `amend_each` and `ChainChange`
go: nothing changes an entry after it was added. The journal is `Stored<E>`: `changes()` is the
nodes added since `mark_stored`, those whose `active_child` changed since (a head move marks
them), and the head - `None` if nothing did. `apply(Stored<E>)` upserts the nodes by digest
(a known digest may only change its place, a new one must follow a node that is there) and sets
the head; `from_stored` is `apply` on an empty chain, so that the checks live once. Idempotent as
before. A node is stored with `entry` nested, not flattened: an entry is an enum from now on.

Each content is one entry type, an enum of what can happen, with inherent methods on its chain
(allowed, since `Chain` is this crate's) and a type alias for the name:

| Content | Entry | What it adds to the chain |
|---|---|---|
| `Transcript = Chain<TranscriptEntry>` | `Message(Message)`, `GivenUp { end: Digest, given_up: GivenUp }` | `push(message)`, `extend`, `messages()`, `given_up_turns()` |
| `MessageHistory = Chain<HistoryEntry>` | `Said { message, context }`, `Reclassified { message: MessageId, classification }`, `Fitted { channel }` | `push(message, context)`, `extend`, `messages()`, `message(id)`, `missing_from`, `last_message_id`, `reclassify`, `fit_into` |
| `NoteChain`, `MemoryChain = Chain<LedgerEntry<K>>` | `LedgerEntry { change: Change<Id>, origin, after, written_at }` | `push`, `current()`, `next_id()`, `with_staged()` |

`Ledger<K>`, `Transcript` and `MessageHistory` as structs, `MessageEntry`, `push_message`,
`messages_of` and `last_message_id_of` go. `Change<Id>`, `Item<K>`, `Staged<K>`, `GivenUp`,
`GivenUpTurn` and `LedgerKind` stay what they are. `Kept<V>` and `Journaled` stay; the state
enums carry `Stored<E>` where they carried `Vec<ChainChange<E>>`, at most one per chain and put.

### The state is the fold

- **History.** `messages()` folds over `all_entries()` in the order they were added - which is
  the order things happened, on every branch - and keeps those on the active path: a `Said`
  adds its message, a `Reclassified` sets the classification of the message it names, a `Fitted`
  fits every message so far. That is what `reclassify` and `fit_into` did in place "on whichever
  branch", now as the record of when. `message(id)` folds the same way for one message - what
  `reclassify_message` asks for the author and the current classification, and what
  `classification_of` in the views answers. `missing_from` and `last_message_id` look at `Said`
  entries only.
- **Transcript.** `messages()` is the `Message` entries of the active path. Giving a turn up moves
  the head back, then pushes `GivenUp { end }` naming the dead branch's last node, then the
  message that tells the agent what ran, as today. `given_up_turns()` finds every `GivenUp` on
  any branch and takes the last `turn_entries` nodes of `prefix(end)`. The model is given
  messages only: `ContextResolver::assemble` and the `StateContextProvider` skip what is no
  message, and a prompt part is placed after a message node only.
- **The digest.** A `Reclassified`, `Fitted` or `GivenUp` entry has a scheme of its own
  (`caelumora.history-reclassified.v1` over the predecessor, the message id and the
  classification; `…-fitted.v1`; `caelumora.transcript-given-up.v1` over the predecessor, `end`,
  the reason and the count, not the time - as a ledger leaves `written_at` out). A `Said` and a
  `Message` keep `message_chain`, so every digest recorded so far still names what it named. A
  digest names a prefix of the record; the transcript's changes when a turn is given up, though
  the model saw nothing new, and `ContextRef`s resolve as before. AGENT-TURN.md says so.
- **The turn under way.** `TurnEntry` holds a `Node<TranscriptEntry>` with its `ContextRef`;
  `TurnState::push` makes the node as the chain would. `TranscriptView::prefix` answers nodes.

### Steps

1. **The wrappers go.** `Transcript`, `MessageHistory` and `Ledger` become aliases with inherent
   impls; the generic primitives become `pub(crate)`; `MessageEntry` and its helpers go into the
   two `push`es. The stored form loses its one level of nesting. No behaviour change.
2. **The history records reclassification.** `HistoryEntry` becomes the enum; `reclassify` and
   `fit_into` push; `messages()` and `message(id)` fold; the readers in `channel.rs`, the views
   and `recorded_context_of` follow. The digest covers a reclassification from here on.
3. **The transcript records a turn given up.** `TranscriptEntry` becomes the enum; `give_up_turn`
   pushes; `given_up_turns`, `TranscriptView`, `assemble`, the context provider and
   `next_turn`'s `see_message` follow.
4. **The container owns the digest, and its stored form is its journal.** `Node` gets `digest`,
   `ChainEntry` goes, `amend`, `amend_each` and `ChainChange` go, `Stored` is public and what
   `changes` tells and `apply` takes, `from_stored` is `apply`; the four state enums carry
   `Stored<E>`. The journal tests of 193's step 2 are rewritten against `Stored`.
5. **Docs.** AGENT-TURN.md: the chain section and what a digest names; STORAGE.md: what a store
   appends; MEMORY.md where it names the ledger's chain. Issue 193's plan is updated where it
   says `ChainChange` and `Amended`.

Each step is a commit of its own and leaves the workspace green. Steps 2 and 3 are the ones with
behaviour: the record of a channel and of an agent holds more than before, and two digests change
meaning. Step 4 is the removal the whole thing is for.

### Decided and open

- `Stored<E>` is the name for what a chain is stored as and tells a store: both are the same
  thing, and the name says what the value is, not what it is for.
- `GivenUp` is an entry of the transcript, not of the history: a turn of internal steps alone
  has no branch in the history, and the transcript is where the turn's messages are.
- `active_child`, which only `leaf_from` reads (issue 08), is kept for now, at the price of
  re-stating a node in the journal when a head move changed its place. Dropping it would make
  the journal "new nodes and the head" alone, and a switch to a branch go to its newest leaf;
  that is issue 08's to decide.

## History

- **2026-10-02** — Opened from the review of issue 193's steps 2 and 3: the journal needed an
  `Amended` variant only because the message chains change entries in place, which the ledger
  never does. The maintainer asked for one class of change and one container for trees, and for
  the names to stop multiplying. Issue 193 is paused until this is done.
- **2026-10-02** — Step 1 done: `Transcript`, `MessageHistory` and `Ledger<K>` are aliases of
  `Chain<…>` with their content's methods as inherent impls; `push_with`, `amend` and
  `amend_each` are `pub(crate)`; `MessageEntry` and its three helpers are gone. The runtime did
  not change: what it called on the wrappers it now calls on the chain.
- **2026-10-02** — Step 2 done, taken out of the plan's step 4 and done before the content steps,
  so that the readers of entries change once: the container owns the digest. `Node<E>` is public
  with `digest`, `parent` and `entry`, `ChainEntry` is gone, `push_with` takes the entry and its
  digest scheme, `entries`, `prefix`, `last` and `children` answer nodes, and a turn's pending
  messages are nodes too (`TurnEntry::node`). The content types lost their `digest` field.
- **2026-10-02** — Step 3 done: `HistoryEntry` is `Said`, `Reclassified` or `Fitted`; a
  reclassification and a lowering are recorded as entries, covered by the digest
  (`digest::history_chain`), and `messages()` and `message(id)` fold over every branch in the
  order things happened. `amend_each` had no caller left and is gone.
- **2026-10-02** — Step 4 done: `TranscriptEntry` is `Message` or `GivenUp`; giving a turn up
  moves the head back and records it as an entry naming the dead branch's end, covered by the
  digest (`digest::transcript_chain`, without the time), and `messages()` is what the model is
  given. With no caller left, `amend` and `ChainChange::Amended` are gone: a chain appends and
  moves its head, nothing else.
- **2026-10-02** — Step 5 done: `Stored<E>` is public, what a chain tells a store since it was
  last stored, and what `apply` merges, checked whole before anything changes; `from_stored` is
  `apply` on an empty chain; `ChainChange` is gone. `active_child` is recorded only where the
  branch last taken is not the newest - so an entry appended changes nothing before it, and the
  journal of a message is that message alone - and `leaf_from` follows the newest child where
  nothing is remembered. The four state enums carry `Stored<E>`, at most one per chain and put.
- **2026-10-02** — Step 6 done, and the issue with it: STORAGE.md says what a state tells a
  store, AGENT-TURN.md what the chain is and what each content's entries are, and issue 193's
  plan carries a note on what 197 revised. What remains of the plan's types: `Chain`, `Node` and
  `Stored` in `chain`; `TranscriptEntry`, `HistoryEntry` and `LedgerEntry` with `Change`, `Item`
  and `Staged`; `Kept` and `Journaled` with the four `…StateChange` enums.
