# 378 — Epic: resource management

**Status:** open
**Area:** Maintenance

## The problem

What the server holds in memory grows with its use and is never given back. A channel's history
and an agent's history and transcript are loaded whole and kept whole for as long as the server runs
([376](376-histories-held-whole-in-memory.md)); every channel of every session that was started
keeps its loop, its actors and its state until the session is stopped, however long nobody speaks in
it; an agent's controller and state stay as long as anything holds them; the prompt texts and tool
sets an agent was given are kept once per turn, for the record. A server meant to run for months
with long-running channels therefore grows for months. This issue is the bracket around the issues
that reduce what is held, and their order.

## Why it matters

- Memory is the one resource that grows with the length of a channel times the number of agents in
  it, and nothing bounds it today; the agents are meant for long-running channels.
- Starting a channel or an actor reads its whole past, so a restart of a long-lived server is slow
  in proportion to how much it did.
- The issues interlock: paging a history ([218](218-paged-histories.md)) needs stable positions
  in a history and reads by range, which [379](379-history-stored-as-it-stands.md) provides;
  unloading a channel ([381](381-inactive-channels-held-in-memory.md)) is pointless while loading
  it again reads its whole past.

## The frame

Decided with the maintainer on 2026-10-16, after weighing options on 376:

- **The chain is the truth.** A history and a transcript stay chains of entries, only appended to,
  with a digest per entry; a change to a message is an entry after it, and what stands is read by
  folding. Nothing here revises [197](197-one-chain-one-model-of-change.md).
- **History and transcript serve different purposes and get different solutions.** The history is
  written for the record and read almost only by the web frontend, in pages, as the messages stand
  today; its chain is folded once, into a derived, persistent store of the items as they stand,
  which is rebuilt from the chain at any time (379). The transcript is what the model is given: its
  active branch is held in memory whole, its older branches lie in the store and are read only for
  the context of an old message ([380](380-transcript-window.md)).
- **Each gets an entry point of its own in the store** - a `HistoryStore` and a `TranscriptStore`
  beside the `StateStore`, implemented by both backends - rather than one generic chain store that
  would carry both sets of needs. What the JSON store cannot do efficiently it may do slowly:
  Postgres ([04](04-postgres-state-store.md)) is where the reads by range and the indexes belong.
- **Everything the holder needs between turns is a small part of the state** - a running token
  total, the classification an external agent's remote side has seen, the length and head of each
  chain - not a sum over a whole chain.

## Recommended order

1. [376](376-histories-held-whole-in-memory.md), narrowed to the refactoring that prepares the
   rest without a change of behaviour: a stable position per entry, reads by range on `Chain`, the
   sums over whole chains turned into small parts of the state, every reader that needs only the
   end or one entry taken off `all_entries()`.
2. [218](218-paged-histories.md) on the positions and ranges of step 1, while the chains are
   still whole.
3. [379](379-history-stored-as-it-stands.md) - the `HistoryStore` with the derived store of items;
   an architecture decision on its contract first.
4. [380](380-transcript-window.md) - the `TranscriptStore` and the window over the active branch.
   After 379, so that the two stores are built alike where they are alike.
5. [383](383-prompt-texts-and-tool-sets-kept-per-turn.md) - once 380 reads old branches from the
   store, the prompt texts and tool sets they refer to can lie there too.
6. [381](381-inactive-channels-held-in-memory.md) - unloading a channel pays once loading it again
   is cheap.
7. [382](382-agents-held-without-a-session.md) - after 381, which decides what still holds an agent.

## Considered and not taken up

- **Folding old pages at read time from an index of the change entries:** a page far back would
  still read every change recorded after it, and the changes grow with the editing without a bound.
  The derived store of 379 reads exactly the page.
- **The ledgers of notes and memory:** chains too, but they grow with what an agent writes, not with
  the channel, and are read whole by every turn. Left out of the epic.

## Done when

Every issue of the order above is `done` or `rejected`, and this one records what changed in the
order along the way.

## History

- **2026-10-16** — Created at the maintainer's request, from the discussion of options on 376: the
  frame above was decided there, 376 narrowed to the preparatory step, 379 to 383 opened.
- **2026-10-16** — 218 done. [223](223-full-inbox-drops-events.md) left the epic: catching up
  from the history, which put it here, was rejected - the moderator decides who hears what - and
  the inbox is to be persisted instead, with [385](385-inbox-lost-on-restart.md); that needs no
  range reads and reduces nothing held in memory. Its plan is recorded on 223 and waits for 379,
  whose store contract decides how a history entry and the deliveries it causes are stored
  together. Step 2 of the order is 218 alone.
- **2026-10-16** — 379 done: a page of a history is read from the store in the cost of the page,
  and the channel loop and an agent's actor hold a window of their history. Its store contract - the
  put of a state writes the chain and the document, the `HistoryStore` is the read side - is what
  380 builds the `TranscriptStore` alike to.
- **2026-10-16** — 380 done: an agent's actor holds the active branch of its transcript, every older
  branch is read from the store by the `TranscriptStore`, built alike to the `HistoryStore`; both
  stores keep the two chains by one implementation. Next in the order: 383, whose prompt texts and
  tool sets are what `AgentContextView::context_of` still takes from the loaded state.
