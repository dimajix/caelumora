# 396 — One message as it stands is read three ways, two of them by folding everything after it

**Status:** done
**Area:** Code structure

Part of [391](391-history-and-transcript-review-epic.md).

## The problem

Three functions answer "the message `id`, as it stands":

- `History::message(id)` (`caelumora-model/src/history.rs`) finds the entry that said it, then
  folds the whole range from there to the end with `history::fold` - which clones every message
  said after it and builds a map of them - and takes the first. It is called by `is_changed`
  before every `modify`, `revoke` and `reclassify` of a history, and by the agent's actor when it
  answers again.
- `Transcript::standing(id)` (`caelumora-model/src/transcript.rs`) reads the whole active path
  with `as_they_stand`, which collects every change on the path into a map and clones every
  message up to the one found. It is called before every `modify`, `revoke` and `reclassify` of a
  transcript.
- `transcript::known(nodes, id)` finds the entry that holds the message and applies to that one
  message the changes that name it, cloning nothing else. The stores use it
  (`TranscriptStore::message`).

The third is what the other two should be. `HistoryEntry::fold_into` already checks that a change
names the message it is applied to and handles a fitting, so `History::message` is: find the entry
that said it, clone the message, apply `fold_into` of every later entry. `Transcript::standing` is
`known(self.entries(), id)` with a message taken back filtered out.

## Why it matters

- A change of one message costs reading and cloning everything said after it, in the history, and
  everything given before it, in the transcript - on every modification, revocation and
  reclassification, in memory, with the window of [379](379-history-stored-as-it-stands.md) as the
  only bound.
- Three functions for one question are three places to keep right; `known` and `standing` differ
  in nothing but what they return for a message taken back.

## Considerations

- After this, `history::fold` is read by `said_in_turn` and `catch_up` alone - ranges of many
  messages, where folding them together pays - and `as_they_stand` by `messages`, the
  `ContextResolver`, `compact` and `retried`, which read whole paths.
- Depends on [395](395-message-change-applied-twice.md) only in that the one rule it introduces is
  what `known` and `History::message` apply; it can be done before, on the two rules as they are.

## Done when

- Reading one message as it stands clones that message and applies the changes that name it,
  nothing else, in the history and in the transcript.
- `Transcript::standing` is `known` with one filter, or gone.

## History

- **2026-10-16** — Opened from the review of the history and the transcript
  ([391](391-history-and-transcript-review-epic.md)).
- **2026-10-16** — Done. `History::message` clones the message where it was said and folds every
  later entry into it (`HistoryEntry::fold_into`, which changes it only if it names it or fits the
  channel); `Transcript::standing` is `known` over the active path, without a message taken back.
  Both chains read one message the same way now, and `fold` and `as_they_stand` are left to ranges
  and whole paths. What *Why it matters* says of the cost was put too strongly: the readers run on a
  change of a message and on answering again, which are rare, over a window of a few hundred
  entries. The gain is one way of reading a message instead of three, not time. No stored data
  changes.
