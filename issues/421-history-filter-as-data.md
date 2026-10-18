# 421 — What a page of the history keeps is a closure no store can filter by

**Status:** done
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

Decided 2026-10-18:

- **The existing query filter, not a type of its own.** `Filter<F: Field>` of the entity stores'
  queries (issue 293) is already a condition as data over paths into a serialized document, made
  for a `WHERE`. A history is filtered by `Filter<HistoryField>`: the fields of an item as it is
  recorded (`Recorded`) - the message said, its sender, role, `internal`, content, mentions, level
  and compartments, and `Element` for an element of a collection itself. `HistoryStore::page`
  takes `Option<&Filter<HistoryField>>` instead of `Keep`; `Keep` and `every` are gone. The
  conditions callers need are constructors on `Filter<HistoryField>`: `said`, `said_by`,
  `written`, `containing`, `heard`, `mentioning`, `readable`. The filter is judged on the item as
  recorded, before its form for the reader is chosen (`Recorded::as_seen_by`) - the same, as
  `Message::published` changes none of the fields filtered by.
- **What may be copied is data the policy answers (B1).** `ConfidentialityPolicy::may_copy`, a
  decision per entry, becomes `copyable(context, channel, session, into) -> Reading`: `Whole`,
  `Released(Classification)` or `Nothing` - which classifications, not each one. `Reading`, how
  far a reader reads, is the reader's relation to a channel: the bound is the reader's, the channel
  decides whether it holds (its session's consent) or is not needed; the name says so, where
  `Copyable` would be read as a property of the channel - and as a marker like `Copy`.
  `may_copy_information` judges a place by it (`Reading::admits`), so there is one rule.
  `channel_reading` asks once and answers the `Reading` a channel view keeps - it replaces
  `ChannelReading` -; a page of the history turns it into a condition over level and compartments
  (`Filter::readable`), held to the flow rule by a test over samples. Rejected: asking the policy
  for each classification found in a channel (B2) - free policies, but a store method of its own
  and set equality of objects in `FieldValue`, for less clarity. The cost: a policy defined outside
  the binary (issue 24) decides as a bound in the lattice of classifications, as the flow rule
  does, no longer freely per classification.
- **Words are compared as names are.** `Filter::Contains` folds both sides as `NamePrefix` does -
  case, accents, runs of whitespace -; a database compares the folded forms with `LIKE '%…%'`.
  For the search of `read_history`, this widens what matches by accents and whitespace.
- **No flag for what is published.** Whether anything of an entry is for others
  (`HistoryEntry::is_published`) was decided in issue 417, step 7: derived from the item as it
  stands, which a database writes as a column with the item.

## History

- 2026-10-18: Opened from issue 417, step 7.
- 2026-10-18: Done as decided. `FieldValue::Bool`, `Filter::Contains` and the constructors
  `all_of`, `nothing`, `negated` added to the query filter; `HistoryField` and the history's
  conditions in `store::history`; `seen` and `page_of` fallible, as judging an item serializes it;
  the memory store stops reading at `from`. `Reading` in `confidentiality`, with
  `Compartments::readable_by` for the condition; the test policies answer it, the `Recorder`
  recording a copy as allowed if anything may be copied. `ChannelView::reading` answers the
  `Reading`. Stored data is unchanged.
