# 380 — A transcript holds every branch it ever had in memory

**Status:** open
**Area:** Persistence

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

## History

- **2026-10-16** — Opened from the discussion on 376, as part of the epic
  [378](378-resource-management-epic.md): the transcript's half of 376.
- **2026-10-16** — [379](379-history-stored-as-it-stands.md) is done, and sets what this issue
  builds alike: the chain is kept apart from the state's document and written by its put
  (`#[serde(skip)]`, `window_from`), the read side is a trait of its own beside the `StateStore`,
  handed out by the `StoreBackend`, and the holder lets go of what lies before its window after a
  put (`Chain::keep_from`). The transcript's window is its active branch.
