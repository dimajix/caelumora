# 376 — Histories and transcripts are held in memory whole

**Status:** open
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

## History

- **2026-10-15** — Issue created while weighing options for
  [223](223-full-inbox-drops-events.md): catching up from the channel's history only moves the
  problem while that history is held whole, and [218](218-paged-histories.md) pages what is sent
  but not what is read. Proposed order: stable positions and a range API as a refactoring first,
  218 and 223 on it, spilling behind it.
