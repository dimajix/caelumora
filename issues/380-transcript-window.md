# 380 — A transcript holds every branch it ever had in memory

**Status:** done
**Area:** Persistence
**Epic:** [378](378-resource-management-epic.md)
**Summary:** An agent's transcript holds every branch it ever had in memory, though only the active branch is given to the model and an old branch is read for the context of an old message alone.

## The problem

An agent's transcript (`Transcript`) is what its model is given: its active branch, from the root
to the head. Every compaction and, for an external agent, every call starts the active branch anew
(`start_anew`), and the branch before stays - nothing is ever removed, so that every digest an agent
recorded still names what the model was given. All of these branches sit in one `Vec` in memory
and are loaded whole when the actor starts, though only the active branch is given to the model,
and an old branch is read for one thing only: the context of an old message, through
`ContextResolver` and `Transcript::prefix`.

## Why it matters

- The active branch is bounded by the model's context size; the branches before it are not bounded
  at all, and grow with every compaction and every call of an external agent.
- Two sums go over every branch: `usage` adds the tokens of every message the agent ever produced,
  and the external agent's actor records every classification its remote side was ever given
  (`work.see`). Both would read what is no longer in memory.

## The frame

From the epic ([378](378-resource-management-epic.md)): the active branch is held in memory whole,
the older branches lie in the store and are read by range when a context asks for them. Different
from the history ([379](379-history-stored-as-it-stands.md)): no pages, no derived store - what
`as_they_stand` does on a loaded branch is all the folding a transcript needs.

## Considerations

- **A branch is a range.** The head of a transcript moves only to the root, so every branch is a
  contiguous range of the order entries were added, and the active branch is a suffix of it.
  `prefix(digest)` of an old context is the range from that branch's root to the entry - a read by
  range, no walk over parents. Should branching ever move the head elsewhere ([08](08-branching.md)
  envisaged it), the store's operation is still "the path to a digest"; only its implementation
  changes.
- **The window** is the active branch, loaded from its root when the actor starts, trimmed after
  the chain is stored: nodes before the root of the active branch are dropped once nothing pending
  refers to them. `Chain` has to hold a suffix for this (a `base` position; `index`, `path` and
  `newest_child` covering the window only), and `apply` to accept a parent below the window as
  older and the store's. Shared with 379 and prepared in
  [376](376-histories-held-whole-in-memory.md).
- **The `TranscriptStore`**, a trait beside the `StateStore` and the `HistoryStore`: append
  entries, the path to a digest, the message of an id on whichever branch (for an external agent's
  notices and `AgentContextView::saw`). Built after 379, alike where the two are alike - how
  entries are appended, how a range is read - and otherwise only what the transcript needs.
- **The sums become small parts of the state.** `usage` a running `TokenUsage` in
  `AgentChannelState`, counted once per message as it is today; the classification the remote side
  has seen a field of `RemoteState`. Both are moved in 376, before the window exists.
- **Deduplication by digest** (`push_with`) sees only the window. A root digest that recurs after
  `start_anew` - the same first message given again - becomes a second node where it was one. The
  same digest names the same content after the same predecessors, so what `prefix` gives is the
  same; the store must not make digests unique.
- **What the context view loads.** `AgentContextView::context_of` resolves an old context against
  the whole transcript today. It loads the branch the context names from the store first and
  resolves against that; the prompt texts and tool sets it refers to are
  [383](383-prompt-texts-and-tool-sets-kept-per-turn.md).

## Done when

- An agent's actor holds its transcript's active branch in memory and nothing older; starting it
  reads that branch, not every branch.
- The context of an old message is resolved from a branch loaded from the store.
- `usage` and the classification seen by an external agent's remote side are read from the state,
  not summed over the transcript.
- AGENT-TURN.md and STORAGE.md say what is held and what is loaded.

## Plan: the store holds every branch, the actor the active one

Decided with the maintainer on 2026-10-16. Built as 379 built the history: the chain is written by
the put of the state that holds it, read by a trait of its own, and the holder lets go of what lies
before its window after a put. What the actor reads of its transcript is the active branch, in
memory; what lies on an older branch is read from the store, by the three readers that ask for it.

### 1. One way to keep a chain, two read sides

The history and the transcript are kept alike, but read differently: the history in pages of its
derived items, the transcript as the path to a digest, without anything derived. A `ChainStore<E>`
trait common to both would hold `entries` alone, which no reader of a transcript needs. *Weighed
and rejected* for that reason; there are two traits for reading, built alike - a key, `StoreResult`,
handed out by the `StoreBackend`, as fresh as what was put.

What is alike is how a chain is kept by position: new nodes appended, a node changed in its place
(`active_child`) replaced, the head written, a suffix loaded from a position. That is one generic
implementation per backend, over the type of entry: in the JSON store a module
`store/json/chains.rs` (segments, head, appending, loading, the repair after a crash), with the
history's items laid over it; in the `MemoryStore` a list of nodes by position, `KeptChain<E>`. The
`MemoryStore` keeps the history that way too, instead of a whole `Chain` merged by `Chain::apply`:
the transcript cannot be kept so (below), and two ways of appending would have no reason.

### 2. A digest may be there twice

`push_with` takes an entry whose digest is there already as that entry - but only within the
window. A root digest that recurs after `start_anew` becomes a second node in the store where the
whole chain had one. The same digest names the same content after the same predecessors, so either
node gives the same path; the store keeps both and makes nothing unique. `path` takes the newest
node with the digest and follows its parents, each the newest node with that digest before its
child. A window loaded from its start holds none twice: a duplicate arises only of a node that lay
before the window when it was pushed, and the window never moves back.

### 3. The transcript apart from the document

`AgentChannelState::transcript` becomes `#[serde(skip)]`, as the history is;
`AgentChannelStateChange::Transcript` goes. The put appends the transcript's pending nodes under
the agent's key, before the document; the get loads the document and then the transcript from
`transcript_from` on (`Chain::suffix`). The JSON store keeps it in `<key>.transcript/` beside the
state's snapshot - `chain-<from>.json` and `head.json`, no items - and deletes it with the state.

`window_from` becomes `history_from` - in `ChannelState` and `AgentChannelState`, with its change
`HistoryFrom` - since an agent's state now has two windows; `transcript_from` and its change
`TranscriptFrom` are new.

### 4. The `TranscriptStore`

A trait beside the `HistoryStore`, for an agent in a channel:

- `path(agent, channel, digest) -> Option<Vec<Node<TranscriptEntry>>>`: the path from the root up
  to the entry with the digest, on whichever branch - what the context of an old message names.
- `message(agent, channel, id) -> Option<Message>`: the message as the transcript holds it, on
  whichever branch, as it was first recorded with every change of it since - what
  `Transcript::message` gives today, over every entry.
- `given(agent, channel, ids) -> HashSet<MessageId>`: those of `ids` the transcript holds, on
  whichever branch - whether what the agent heard was passed on to it (issue 196).

The JSON store reads its segments backwards for each; slow where a database's indexes are for it,
as for the history.

### 5. The readers of older branches go to the store

- **The context of an old message.** `AgentContextView::context_of` loads the path its reference
  names from the store, always, and resolves against it. The `ContextResolver` stays in the model
  and loads nothing: `ContextResolver::of(state)` resolves against the window, as the turn and
  `next_context_tokens` do - their reference is the head of the active branch -, and
  `ContextResolver::on_path(state, &path)` against a path loaded before. The prompt texts and tool
  sets still come from the state, until 383.
- **Whether a message reached the transcript.** The REST page of an agent's history asks once per
  page, `given(ids)`, instead of `in_transcript` per message over every branch.
- **A change told to an external agent's remote side.** `tell` reads the message as it was given
  from the store (`ActorRuntime::transcript_message`, the runtime holding the `TranscriptStore` as
  it holds the `HistoryStore`): it lies on a branch before the active one almost always. The hand
  over stays a function pointer, now returning a future and given the runtime -
  `for<'a> fn(&'a ActorRuntime, &'a mut AgentChannelState, TranscriptEntry) -> BoxFuture<'a, ()>`
  - so that the body knows nothing of the kind of agent and a notice keeps its place among what was
  heard. Whether a notice is pending is still read from the active branch, in memory; a store that
  cannot be read is logged, and nothing is told. *Weighed and rejected:* a trait for the hand over
  (one more type for two behaviours, one of them empty); the changes collected and told after the
  batch (a notice would follow what was heard after it, against 236); `RemoteState` keeping what was
  given (unbounded, what this issue removes).

### 6. The window

The window is the active branch, exactly, with no least size: every reader in memory reads only it -
a turn, a compaction, `retried`, `answers_again_after`, `take_in`, `classifications_given`,
`next_context_tokens`. `AgentChannelState::keep_window` also keeps the transcript from the root of
its active branch on (`Chain::keep_from`) and records `transcript_from`; right after `start_anew`
the window is empty. The active path lies within it whole: a node always comes after the one it
follows.

### What is documented

AGENT-TURN.md, *What is held in memory*: the transcript's window, and that the context of an old
message is read from the store. STORAGE.md: the transcript beside the history, the common way a
chain is kept, a digest twice, the shape of the tables. `AgentContextView::transcript` says it is
the active branch.

### Steps

1. The common way of keeping a chain (`chains.rs`, `KeptChain`), the `MemoryStore`'s history on it,
   `window_from` renamed `history_from` - without a change of behaviour.
2. The transcript apart from the document, the `TranscriptStore` in both backends, its tests in the
   store suite.
3. The readers of older branches through the store: `context_of`, `given`, `tell`.
4. The window.
5. The documents; this issue done.

Every stored state has to be deleted afterwards.

## History

- **2026-10-16** — Opened from the discussion on 376, as part of the epic
  [378](378-resource-management-epic.md): the transcript's half of 376.
- **2026-10-16** — [379](379-history-stored-as-it-stands.md) is done, and sets what this issue
  builds alike: the chain is kept apart from the state's document and written by its put
  (`#[serde(skip)]`, `window_from`), the read side is a trait of its own beside the `StateStore`,
  handed out by the `StoreBackend`, and the holder lets go of what lies before its window after a
  put (`Chain::keep_from`). The transcript's window is its active branch.
- **2026-10-16** — The plan decided with the maintainer: the transcript kept as the history is, by
  one generic way of keeping a chain per backend, read by a `TranscriptStore` of its own; the
  window the active branch; the context of an old message, whether a message reached the
  transcript and what an external agent tells its remote side read from the store.
- **2026-10-16** — Done, in the five steps of the plan. An agent's actor holds the active branch of
  its transcript (`transcript_from`, `keep_window`); the store holds every branch, written by the
  put of the agent's state, read by the `TranscriptStore` (`path`, `message`, `given`). The context
  of an older message is resolved against a path loaded from the store
  (`ContextResolver::on_path`), a page of an agent's history asks once which of its messages reached
  the transcript, and an external agent reads what its remote agent was given before it tells of a
  change. Where it differs from the plan:
  - The history's items keep how far they are derived in a file of their own, `items-head.json`,
    so that `head.json` is the same for every chain; the order of writing and the repair stay.
  - Both stores check that what a put appends fits before they write anything; the `MemoryStore`
    checks positions alone, as the JSON store always did, no longer the parents a `Chain` checks.
  - `Transcript::message`, which read every branch in memory, is gone; its rule is `known`, which
    the stores read by, and `TranscriptEntry::changed` is public for the hand over.
  - `tell` reads from the store and hands what it read to `give_notice`, the part tested without a
    runtime.
  - `AgentContextView::in_transcript` takes the messages of a page and answers which reached the
    transcript; `AgentContextView::of` takes the agent and the channel instead of a `HistoryKey`.
  - The `TranscriptStore` has no `entries`: no reader needs the chain itself.
  - `AtomicU8::fetch_update`, deprecated by the toolchain, became `try_update` on the way, since the
    hook refuses a commit while clippy warns.

  Every stored state is to be deleted: the transcript is no longer part of the agent's document,
  `window_from` is `history_from`, and the history's head no longer holds `items_end`.
