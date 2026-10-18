# 430 — A summary is given the times of messages in UTC, not in the session's time zone

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** Context and knowledge
**Summary:** `Turn::context_until`, which hands a component the transcript to work on - the summary of a compaction -, takes the turn's profile but not its time zone, so the model that summarizes reads the times of messages in UTC while the turn's own model reads them in the session's zone.

## The problem

Since [424](424-time-in-the-prompt.md) the marker of a message of someone else says when it was
written (`ModelProfile::message_times`), in the time zone the turn began with
(`ContextRef::time_zone`, the session's). `Turn::context_until`
(`caelumora-agents/src/turn/context.rs`) builds a reference of its own for the transcript up to an
entry:

```rust
ContextRef { transcript: Some(last), profile: self.current().work.context.profile, ..ContextRef::default() }
```

It takes the profile of the turn, so the messages are framed as the turn's model is given them -
but its time zone is `TimeZone::default()`, UTC. Its one caller is the summary of a compaction
(`SummaryCompaction`, `caelumora-agents/src/compaction/summary_compaction.rs`): the model that
summarizes is given every time in UTC. `TimeZone::format` writes no zone, so nothing tells it the
times are not the session's.

## Why it matters

A summary that keeps a time - "on Tuesday at 14:00 Alice asked ..." - keeps it in UTC, and the
turn's model, which reads every other time in the session's zone, takes it for one of its own. In
a session far from UTC that is hours off, and may be another day. Nothing fails; the summary is
only wrong.

## Considerations

- **A: the turn's time zone taken over beside its profile.** One field more in
  `context_until`; what the summarizing model is given is framed exactly as the turn's.
- **B: the summary told the times without a zone, in UTC, and that they are.** Would make the
  summarizing model convert them; more prompt, and a model converting times is what 424 avoided.

A is the fix: `context_until` is to give the transcript "as the model of the turn would be given
it", and the time zone is part of how it is given. With [426](426-context-ref-kept-once.md) the
profile and the time zone are both fields of the `FrameRef`; whichever lands second takes over
the other's shape.

## Done when

- `Turn::context_until` gives the times of messages in the turn's time zone.
- A test of the summary's input shows a message's time in a session's zone other than UTC.

## History

2026-10-18: Opened from the work on 426, where `context_until` was rewritten for the `FrameRef`
and was seen to take the profile alone.

2026-10-18: Done as option A. `Turn::context_until` takes the turn's time zone beside its profile
from its `FrameRef`; the test `the_times_of_messages_are_summarised_in_the_time_zone_of_the_turn`
(`summary_compaction.rs`) shows the summarising model a message's time in Europe/Berlin.
