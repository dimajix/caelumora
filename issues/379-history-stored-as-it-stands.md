# 379 — A history is read by folding its whole chain

**Status:** done
**Area:** Persistence

## The problem

A channel's history and an agent's history are chains of entries (`History`), and what they say is
read by folding: `messages()` and `items()` go through every entry of every branch
(`fold_nodes`) so that each message carries every `Modified`, `Revoked` and `Reclassified` recorded
after it, and a `Fitted` reaches everything before it. A page of the history therefore costs the
whole chain, however far back the page lies and however small it is, and the chain has to be in
memory for it. The chain is written for every message said and read almost only by the web frontend
([218](218-paged-histories.md)), by an actor catching up what it missed
([223](223-full-inbox-drops-events.md)), and by the check before a message is changed - none of
which needs more than a page, a range or one message.

## Why it matters

- A history grows without bound, and holding it whole is what [376](376-histories-held-whole-in-memory.md)
  is about; it cannot be held in part while reading a page needs all of it.
- Paging ([218](218-paged-histories.md)) saves transfer and rendering only, as long as the read
  behind it folds the whole chain.
- An index of the change entries would make a page cost the changes recorded after it instead of
  the entries - better, but still growing with the editing without a bound, and a page far back
  reads all of them (considered on the epic, [378](378-resource-management-epic.md)).

## The frame

From the epic: the chain stays the truth, only appended to, with a digest per entry. Beside it the
store keeps a **derived, persistent store of the items as they stand** - every `Said` with every
change since folded into it, the markers of turns, plan steps, commands, activities and compactions
as `items()` gives them - addressed by the item's running number, with an index by message id. It is
rebuilt from the chain at any time, and the store suite checks that it equals the chain folded.

## Considerations

- **Two numbers per entry.** `seq`, the position in the chain, for every entry, changes included:
  the address in the store, and what an actor remembers it has heard up to (223). `n`, the running
  number of the item, for the entries that add something - what `items()` returns - and not for
  the changes: what a page is addressed by (218), dense, without gaps. The chain hands out both
  when it appends, from two counters kept with the head in the state document; whether an entry is
  an item or a change of an earlier one is the entry type's to say, through a small trait
  (`amends() -> Amends<Key>` with `Nothing`, `One(key)`, `All`), the same answer the store needs.
- **One rule of folding, three users.** How an entry changes a standing message is today the match
  arms of `fold_nodes`; it becomes a function of the model (`HistoryEntry::fold_into(&mut Message)`,
  say) used by the fold over the window in memory, by the store when it applies an appended entry
  to the derived store, and by the rebuild of the derived store from the chain.
- **Appending does two things in the store:** the node goes onto the chain, and the entry is applied
  to the derived store - a `Said` becomes a row, a `Modified` changes the row of its message in
  place, a `Fitted` every row of the chain. The last is a loop over everything, once per
  reclassification of a channel, and may take its time: in Postgres a batched update, in the JSON
  store a rewrite of every segment. Applying `Fitted` at read time instead would split the fold
  again and is not taken up.
- **The `HistoryStore`**, a trait beside the `StateStore` in `caelumora_model::store`, handed out
  by the `StoreBackend` and implemented by `MemoryStore`, `JsonFileStore` and later Postgres:
  append entries, a page of items by number, the message of an id as it stands, the entries after
  a position, rebuild. Its contract is an architecture decision before it is built, together with
  what stays on the `StateStore` - the small parts, the counters and the head of each chain.
- **The JSON store** keeps the chain in segment files of a fixed number of entries, addressed by
  `seq`, and the derived store in segment files by `n`; an index by message id is scanned. Slow is
  acceptable there: Postgres ([04](04-postgres-state-store.md)) is where the indexes belong, and
  this issue decides the shape its tables take.
- **What the holders read from the window** - `last_message_id`, `turn_given_up_last`,
  `said_in_turn`, the check before a change - stays a read of the chain in memory; an item in the
  window is always complete, since every change to it comes after it. `VisibleHistory`, the part a
  participant sees from its `HistoryStart`, becomes a filter over pages that knows the start's
  position.
- **The agent's history** is the same type and gets the same store, under the agent's key; its
  REST endpoint is paged like the channel's (218).
- **An entry and what it causes, stored together.** [223](223-full-inbox-drops-events.md) is to
  put an actor's inbox into the channel's state and store a history entry and the deliveries it
  causes in one write, so that a crash cannot keep the one without the others. With the history
  in a store of its own, that is a write across the `HistoryStore` and the `StateStore`: the
  contract has to say how - one transaction where the backend has one, or an order of writes with
  a repair on loading - before 223 is built on it.

## Done when

- A page of a channel's or an agent's history is read from the store in the cost of the page, as the
  messages stand today, by item number.
- The derived store is rebuilt from the chain by one operation, and the store suite shows it equal
  to the chain folded - after changes, after `Fitted`, after a rebuild.
- `MemoryStore` and `JsonFileStore` implement the `HistoryStore`; the window in memory is bounded
  (376, [378](378-resource-management-epic.md)).
- STORAGE.md says what the chain is, what the derived store is and that the latter is derived.

## Plan: the store holds the chain and the items, the holder a window

Decided with the maintainer on 2026-10-16. Everything a view, a tool or an actor catching up reads
of a history comes from the store; what the holder of a history - the `ChannelLoop`, an agent's
actor - reads comes from a window of recent entries it holds in memory. Two readers, two sources,
no logic that merges them: the store is as fresh as the memory, since a history is stored before
anything of it is routed or answered.

### 1. The store appends; the holder puts

Appending stays inside the put of a state (`put_channel_state`, `put_agent_channel_state`): the
one write of everything that changed, one transaction where the backend has one. *Weighed and
rejected:* an `append` of the `HistoryStore` called before the put - two writes, no transaction
across them, and the atomicity 223 needs would be out of reach in Postgres.

For that the state tells its chain apart from its document. `ChannelState` and
`AgentChannelState` keep their `history`, but it is not part of what `Journaled` serializes or
lists as changes - a suffix cannot be written whole anyway (376, step 6). The typed puts store the
document as today (snapshot and journal) and the chain's pending nodes (`History::changes`) under
the state's `HistoryKey`; the typed gets load the document and then the chain from `window_from`
on (`Chain::suffix`), a small part of the document the holder sets (step 4). The `MemoryStore`
keeps the whole chain and the derived items per key; the `JsonFileStore` as step 2 says.

### 2. The `HistoryStore`: the read side

A trait beside the `StateStore` in `caelumora_model::store`, handed out by the `StoreBackend`,
implemented by both backends; its key `HistoryKey::Channel(ChannelId)` or
`HistoryKey::Agent(AgentId, ChannelId)`:

- `page(key, before: Option<u64>, limit) -> HistoryPage`: the last `limit` items before the
  position `before` - the last ones without it - as they stand, by `seq`. The one address of an
  item is its position in the chain: a page costs the page in both backends, Postgres by an index,
  the JSON store by segments read backwards. *Weighed and rejected:* the running item number `n` of
  the considerations above, decided on with 218 - a second counter in the head and a trait that
  tells items from changes, for a numbering nobody needs dense. `earlier` stays opaque to clients,
  so the REST API does not change.
- `message(key, id) -> Option<(u64, Message)>`: the message as it stands, with its position -
  what the check before a change needs, the classification of a message, where a participant's
  view begins (`HistoryStart`).
- `entries(key, from: u64) -> Vec<Node<HistoryEntry>>`: the chain itself from a position on,
  for an actor catching up, for what a window does not hold, and for 223's deliveries.
- `rebuild(key)`: the derived items anew from the chain.

The store applies an appended entry to the items by `HistoryEntry::fold_into` - the one rule - a
`Said` as a new item, a change to the item of its message, a `Fitted` to every item, which may
take its time. A viewer reading a channel entry by entry filters pages in the view, reading on
until a page is full, as `history_page` does today. The derived items hold every item ever
appended, the window's included, so that a page never needs the memory.

### 3. The readers go to the store

- `ChannelView::history_page` and `message`, `AgentContextView::history_page` and
  `classification_of`, `MemberView` where it reads a history: through the `HistoryStore` the
  `Views` hold, whether the channel runs or not. `ChannelLoop::visible_history` and
  `VisibleHistory` go; what the tool `read_history` and an agent's knowledge record of a history
  (its digest and start) they take from the pages and the document.
- The checks before a change in `ChannelLoop::admission` read `message(key, id)`.
- `catch_up` reads the channel's entries after the position of the last message the agent's
  history has (`message`, then `entries`), instead of `missing_from` over both whole chains; what
  it takes in and how is unchanged.
- The tool `read_history` reads pages backwards until it has its `last` matching messages.
- `History::items`, `messages`, `items_before`, `message`, `position_of` and `missing_from` are
  then the store's; what remains of them in the model serves the store's fold and the tests.

### 4. The window

The holder keeps a suffix of the chain: the `ChannelLoop` from the end minus 200 entries, an
agent's actor from its last `Compacted` on, at least 200 entries. After a put the holder trims the
chain to that and records `window_from` in the document; on load the chain comes from there. What
the holder reads of its window - `last_message_id`, `turn_given_up_last`, `said_in_turn` and
`turn_began`, `produced_on_branch`, the digest that deduplicates an entry pushed again - reads
backwards from the end and ends at the window's start at the latest: a turn and a branch lie in
the window, which AGENT-TURN.md states as the assumption it is. `fit_into` records its entry as
today; the store applies it.

### 5. The JSON store

A directory per state: `state.json` and `state.journal` for the document, as today;
`history/chain-<from>.json`, segments of a fixed number of entries by `seq`;
`history/items-<from>.json`, the items as they stand, by the same positions. Appending reads the
last segment, changes it and writes it through a temporary file, as every file is written;
`Fitted` rewrites every items segment; a message is found by scanning the items backwards. The
order of writes is chain, items, document; loading a document behind its chain - a crash between
the two - takes the chain's end as the truth and repairs the items from it. That is the slowness
and the imperfection the JSON store may have; Postgres ([04](04-postgres-state-store.md)) has the
transaction and the indexes, with tables `history_entries(key, seq, parent, digest, entry)` and
`history_items(key, seq, message_id, item)` beside the documents.

### What is documented

- STORAGE.md: the chain as the truth, the derived items, the window and `window_from`; that a put
  writes the document and the chains; the `HistoryStore` as the read side; the JSON layout and the
  shape Postgres takes.
- AGENT-TURN.md: what the holder reads from its window, and the assumption that a turn and a
  branch lie in it.
- CHANNELS.md: that the history a participant sees is read from the store, in pages.

### Steps

1. The model and the memory store: `HistoryKey`, the `HistoryStore`, the chain taken out of the
   document with `window_from`, `MemoryStore` implementing both the typed puts and the read side;
   the store suite's `history_store_behaves` - the items equal the chain folded after changes,
   after `Fitted`, after `rebuild`. **Every stored state (`state/` of the JSON store) is to be
   deleted.**
2. The JSON store: segments, items, `rebuild`, loading a suffix, the order of writes and its
   repair; the suite run against it.
3. The readers: views, checks, `catch_up`, `read_history`; the model's whole-chain readers
   removed or confined to the fold and the tests.
4. The window: trimming, `window_from`, loading as a suffix; tests of the holder's reads at the
   window's edge.
5. The documents; 379 `done`, 378's order noted, 218's note on `n` answered.

## History

- **2026-10-16** — Opened from the discussion on 376, as part of the epic
  [378](378-resource-management-epic.md): the history's half of 376, with the derived store the
  maintainer proposed.
- **2026-10-16** — [218](218-paged-histories.md) pages both histories by the chain position
  (`seq`) of a page's oldest item, while the chains are whole; the maintainer decided that this
  issue switches pages to the item number `n` once the derived store addresses items by it. The
  position is handed to clients as `earlier` and passed back as `?before=` without their computing
  with it, so the REST API keeps its shape; `History::items_before` and its callers change.
- **2026-10-16** — Architecture decided with the maintainer and the plan above written: the put of
  a state stays the one write, the `HistoryStore` is the read side, a page is addressed by `seq`
  alone - the item number `n` decided on with 218 is not built -, and the holder keeps a window of
  200 entries, an agent from its last compaction on. Implementation to begin on the maintainer's
  word.
- **2026-10-16** — Done, in the steps of the plan. Deviations from it: the `HistoryStore` answers
  `said(key, id)` - the item that said a message, with what it was produced from - where the plan
  had `message`, so that what an agent saw is read from the store too; a page takes a filter
  (`keep`), so that a page of what was released to a viewer is full, and carries the digest it was
  read from; the JSON store keeps a history in `<key>.history/` beside the state's snapshot rather
  than a directory per state. Found on the way: a history held from a position on recorded no
  change of a message before it, since it checked the change against what it held; it records one
  now, and the store applies it. The runtime takes the store of the states as a `StoreBackend`, so
  that the `StateStore` and the `HistoryStore` are the same store. Every stored state is to be
  deleted.
