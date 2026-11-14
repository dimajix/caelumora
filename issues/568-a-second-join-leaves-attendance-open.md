# 568 — A join recorded while a participant's attendance is open leaves it open past their leave

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** Sessions and channels
**Summary:** A channel records a join for a participant whose range of attendance is still open in three cases, which opens a second range; a later leave closes only the last, the first stays open, and what the participant is shown of the history then reaches past their leave.

## The problem

What a participant is shown of a channel's history is read by their ranges of attendance
(`Attendance::of_reader`, `caelumora-store/src/history.rs`), which `Attendance::take` derives from
the history: a `ParticipantJoined` opens a range - from what the `HistoryShared` before it shared,
or from the join -, a `ParticipantLeft` closes the open range of its participant, the last one
that is open. A former participant reads the history as the participant they were
(`ChannelView::reading_as`).

A channel records a join for a participant whose range is still open in three cases, none of
which records a leave first:

1. **One added while listed already** - an agent listed without an actor, left out as its channel
   started since what it runs with was withdrawn: `ChannelLoop::take_in` starts its actor with a
   join (`start_participant` with the one who added it), which `run` records with its
   `HistoryShared`. Kept so by issue [564](564-taking-someone-in-is-rolled-back-in-part.md),
   step 2: in the channel the join is told, since its actor starts only then.
2. **`ChannelLoop::follow_agents`** starts the actor of such an agent once it can run, and records
   a join from where its last range began (`rejoining_from`), as for an agent restored.
3. **An agent deleted while its session was stopped**, restored later: no leave was recorded as
   it was deleted, its channel left it out as it started, and `follow_agents` records its join.

Each opens a second range beside the first. Once the participant leaves, the leave closes the
second; the first stays open to the end. Whoever reads the history as that participant afterwards
is shown what was said after the leave - the agent's own context, for whoever may read it, or a
removed participant who still reaches the channel.

Not yet checked: who can read a channel's whole history as a participant after leaving it - a
user removed loses the channel's group, an agent removed from a channel of a session it stays in
may not -, and so how far what is shown reaches in practice.

## Considerations

- **Attendance keeps one open range** per participant: a join while one is open opens no second,
  and takes over an earlier start where the join's share reaches further back. One place, the
  rule of attendance; the history keeps recording what happened in the channel.
- **The channel records no join while the range is open**: `take_in` and `follow_agents` start
  the actor without a join where the participant was listed and no leave recorded. The history
  then no longer says that the actor came only now, and step 2 of issue 564 is taken back for the
  channel; the third case still needs the channel to know that no leave was recorded.
- Either way, a store keeps the attendance it derived and only takes in what is added
  (`caelumora-store-postgres`, `histories.rs`), so ranges left open already stay open until it is
  derived anew from the history, or the stored data is deleted.

## Done when

- A participant's range of attendance ends where they left, however often a join was recorded
  while it was open; the three cases above are held by tests.
- Whether the history still records such a join is decided and documented in CHANNELS.md.

## History

- **2026-11-14** — Opened from a finding of the review of issue
  [564](564-taking-someone-in-is-rolled-back-in-part.md), step 3b, as the maintainer asked.
