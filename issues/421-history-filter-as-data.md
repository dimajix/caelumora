# 421 — What a page of the history keeps is a closure no store can filter by

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Persistence
**Summary:** `HistoryStore::page` takes `Keep`, a Rust closure, for what a viewer may read and what a search looks for; a database cannot turn it into a `WHERE`, so it has to become data.

## The problem

`HistoryStore::page` takes `keep: &Keep<'_>` - `dyn Fn(&HistoryEntry) -> bool` - and keeps the
items it lets through. Its callers pass:

- `ChannelView::history_page_of`: `may_read_item`, which for a channel read entry by entry
  (`ChannelReading::Released`) keeps the messages whose classification the viewer may read - a
  permission check -, together with what a search keeps;
- the agents' `channel_history` component: a search by sender or word;
- `ChannelView::unread`: the messages of others that are heard, and those mentioning the viewer;
- `start_of` (joining, issue 417): messages only.

A memory or JSON store calls the closure on every item it passes over. A database cannot: the
predicate has to be something it can write as a condition over the row (issue 332 asks the same of
every permission check). Issue 417, step 7, does this for whom an entry went to; what is left is
`Keep`.

## Decision

To be discussed. The direction: a filter as data - a type the store translates - with what the
callers need: only messages, by sender, not by sender, heard, mentioning, containing a word, of a
classification the reader may read. The row holds what it is filtered by (kind, sender,
classification, mentions).

## History

- 2026-10-18: Opened from issue 417, step 7.
