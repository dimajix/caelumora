# 197 — Two models of change over one chain, and a type for every step between them

**Status:** open
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

## History

- **2026-10-02** — Opened from the review of issue 193's steps 2 and 3: the journal needed an
  `Amended` variant only because the message chains change entries in place, which the ledger
  never does. The maintainer asked for one class of change and one container for trees, and for
  the names to stop multiplying. Issue 193 is paused until this is done.
