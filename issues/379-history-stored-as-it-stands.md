# 379 — A history is read by folding its whole chain

**Status:** open
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

## History

- **2026-10-16** — Opened from the discussion on 376, as part of the epic
  [378](378-resource-management-epic.md): the history's half of 376, with the derived store the
  maintainer proposed.
- **2026-10-16** — [218](218-paged-histories.md) pages both histories by the chain position
  (`seq`) of a page's oldest item, while the chains are whole; the maintainer decided that this
  issue switches pages to the item number `n` once the derived store addresses items by it. The
  position is handed to clients as `earlier` and passed back as `?before=` without their computing
  with it, so the REST API keeps its shape; `History::items_before` and its callers change.
