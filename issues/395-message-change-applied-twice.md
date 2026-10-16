# 395 — A change of a message is applied and digested by two copies of one rule

**Status:** rejected
**Area:** Code structure

Part of [391](391-history-and-transcript-review-epic.md).

## The problem

A message once said can be modified, taken back or reclassified
([234](234-modify-revoke-and-retry-messages.md),
[239](239-reclassified-message-not-in-agents-state.md)); the change is an entry after the message,
in the history and in the transcript alike. The rule by which the change is applied to the message
as it stands lives in `Message::modify`, `revoke` and `reclassify` - once. But the step from the
entry to that rule is written twice:

- `HistoryEntry::fold_into` and `HistoryEntry::changed` (`caelumora-model/src/history.rs`) match
  `Modified`, `Revoked` and `Reclassified` onto the three methods and onto the message they name;
- `TranscriptEntry::apply_to` and `TranscriptEntry::changed` (`caelumora-model/src/transcript.rs`)
  match the same three variants onto the same three methods, `apply_to` without the `bool` the
  history needs;
- `digest::history_chain` and `digest::transcript_chain` map the same three variants onto
  `message_modified`, `message_revoked` and `message_reclassified`.

The variants carry the same fields in both enums.

## Why it matters

- A change to the rule - a field a modification takes over, a fourth kind of change - is made in
  four places, and a difference between them is found by a test at best.
- The two entry types read as if they changed messages differently; they do not.

## Considerations

- **A borrowed enum `MessageChange<'a>`** in `message.rs` - `Modified(&Message)`,
  `Revoked(MessageId, DateTime<Utc>)`, `Reclassified(MessageId, &Classification)` - with
  `apply_to(&mut Message) -> bool`, `message_id()` and `digest(previous)`. Each entry type offers
  `fn change(&self) -> Option<MessageChange<'_>>`; `fold_into` keeps only the `Fitted` arm as its
  own, `apply_to` goes, `changed` becomes `change().map(|c| c.message_id())`, and the two
  `*_chain` digest functions call `change.digest(previous)`. The stored form of the entries stays:
  the variants stay flat on both enums.
- **One owned enum as the payload of one variant** `Changed(MessageChange)` on both entry types:
  less code still, but changes the stored form of every change entry. Stored data may be deleted,
  so possible; the borrowed enum reaches the same without it.
- `StandingItems::amend` in `store/history.rs` repeats the distribution of `fold` - a fitting to
  every item, a change to the item it names - and is left as it is: it is the store's reading of
  the history, and after this issue it calls the one rule as `fold` does.

## Done when

- The step from a change entry to `Message::modify`, `revoke` or `reclassify`, and to its digest,
  is written once and used by both entry types.
- A test shows that the same change gives the same digest and the same message in a history and in
  a transcript.

## History

- **2026-10-16** — Opened from the review of the history and the transcript
  ([391](391-history-and-transcript-review-epic.md)).
- **2026-10-16** — Rejected with the maintainer: the gain does not pay for what either form adds.
  The rule itself is written once already - what a change does to a message is `Message::modify`,
  `revoke` and `reclassify`, the three digest schemes are defined once in `digest` -; what stands
  twice is only the dispatch from a variant to its function, three lines on each side. The set of
  changes has been stable since 234 and 239, and a fourth kind would touch the event, the chain
  methods, the intake and the channel loop anyway, beside which the two dispatches hardly count; the
  tests of both chains would catch them drifting apart. The borrowed `MessageChange<'a>` adds a
  type, a lifetime and an accessor and leaves every variant where it is; the owned one as the
  payload of `Changed` deepens every pattern by a level, nests the stored form and has stored
  data deleted. The one imprecision found on the way - the hand-over of an external agent takes a
  whole `TranscriptEntry` where only a modification or a revocation can come (`HandOver`,
  `give_notice`) - is local to that actor and no reason for a type in the model.
