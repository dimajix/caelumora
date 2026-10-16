# 393 — Five wrappers around `History::record` that only assert the variant

**Status:** done
**Area:** Code structure

Part of [391](391-history-and-transcript-review-epic.md).

## The problem

`History` records what is not a message said or a change of one through five methods -
`end_turn`, `start_plan_step`, `record_command`, `record_activity`, `record_compaction`
(`caelumora-model/src/history.rs`) - each of which takes a `HistoryEntry`, asserts in a debug build
that it is the variant its name says, and calls the private `record`. Their callers -
`ChannelLoop::append_history` in `caelumora-runtime/src/channel/run.rs`, `AgentChannelState::
finish_turn`, `give_up_turn` and `compact` - build the entry in full themselves and then pass it to
the method that repeats its name. `Transcript` has the same shape twice: `end_turn`, which asserts
a turn's end, and `push_entry`, which appends any entry as it is.

## Why it matters

- Every new kind of item - the next thing an agent does that the record is to hold - comes with a
  wrapper that says nothing the entry does not say already.
- The one thing the wrappers guard, that a change of a message is not recorded without its check
  (`modify`, `revoke`, `reclassify`), is guarded by none of them: it is `record` being private.

## Considerations

- **One `record(item)` on each chain**, public, with `debug_assert!(item.is_item() &&
  item.said().is_none())` on the history - a message said goes through `push`, with its context -
  and `debug_assert!(entry.message().is_none() && entry.changed().is_none())` on the transcript.
  The callers change only the method's name.
- **`HistoryEntry::of_event`.** `ChannelLoop::append_history` maps every `ActorEvent` that is part
  of the record to its `HistoryEntry`, with the envelope's sender, initiator and time, and
  `AgentChannelState::finish_turn` and `give_up_turn` build the same ends for the agent's own
  history. A constructor in the model, where `ActorEvent` and `HistoryEntry` both lie, would put the
  mapping with the owner of both and leave the channel loop with one call. To be weighed here: the
  gain is one `match` moved, the cost a model type that knows the events.

## Done when

- `History` and `Transcript` each have one method that records an item that is neither a message
  nor a change of one, and no wrapper per variant.
- What may be recorded without a check is said once, where `record` is.

## History

- **2026-10-16** — Opened from the review of the history and the transcript
  ([391](391-history-and-transcript-review-epic.md)).
- **2026-10-16** — Done. `History::record` and `Transcript::record` are public and take whatever is
  no change of a message - on the history `HistoryEntry::is_item`, on the transcript
  `TranscriptEntry::changed` is `None`, asserted in a debug build -, a message said included:
  `push` and `give` are written through them. The five wrappers of the history,
  `Transcript::end_turn` and `push_entry` are gone; a change is appended by a private `append`
  behind the method that checks it, which is what keeps it from being recorded unchecked. Wider
  than the assertion first proposed above, which left a message said out: the one rule "a change
  goes through its own method" is simpler to state than a list of what `record` takes.

  `HistoryEntry::of_event` was weighed and not taken up. With one `record` the arms of
  `ChannelLoop::append_history` are one constructor each; which events are part of the channel's
  record - not a command the session ran, which carries no turn - is the channel's rule, not the
  history's; and `finish_turn` and `give_up_turn` build an agent's ends from its turn, not from an
  event, so there is no second copy of the mapping to fold into it.
