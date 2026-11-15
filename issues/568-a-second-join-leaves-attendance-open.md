# 568 — A join recorded while a participant's attendance is open leaves it open past their leave

**Status:** done
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
- **2026-11-15** — Done by the first consideration: `Attendance::take` keeps at most one open range
  per participant. A join recorded while one is open continues it, its start moved back only where
  the join's share reaches further, and the next leave ends it. The history still records the join,
  since the actor starts only then, as step 2 of issue 564 decided; CHANNELS.md and STORAGE.md say
  so. Not the second consideration: it needed the same special case in `take_in` and
  `follow_agents`, and for the third case a knowledge of leaves not recorded that the channel does
  not have. A test of the store holds the rule, one of the kernel the three cases.

  What the issue left unchecked: a former participant is read as one by `ChannelView::reading_as`,
  for whoever is listed or former and reads the channel as a whole; by `AgentContextView`, which
  reads as its agent; and, for what a user has not read yet, by `ChannelView::unread` and
  `UserController::mark_read`. All are reached only by who may read the channel (`GetChannel`: its
  owner and the group of its participants). A removed user loses the group and reads on as a former
  participant only as the channel's owner. A removed agent reads what its owner may read, so while
  its owner takes part it reads the channel as the former participant it is - in `read_history`
  too -, and whoever may read its context reads it as the agent. The range left open thus showed the
  agent what was said after it left, and a reader of its context what went to every participant
  after the agent's leave - past what that reader was given as a participant themselves, if they
  joined later with less of the history; an owner who left was counted as unread what was said
  after. Ranges already left open in a store stay open until it is derived anew
  (`HistoryStore::rebuild`, which nothing calls outside the store's tests) or the stored histories
  are deleted.
- **2026-11-15** — A finding of the work, fixed on the same branch as the maintainer asked: a
  newcomer granted nothing was given more. In a channel without messages,
  `HistoryAccess::start_in` answered "all of it", as issue [46](46-channel-participants.md) had
  built it in its first step, when a history held messages alone; with messages, it answered
  "after the last message", which shared what was recorded after it - how an agent's turn ended,
  who joined since. A `HistoryShared` was recorded for either, and with the attendance rule above,
  a join while attending moved the start of the open range back to where the share began. Nothing
  granted now begins at the join (`HistoryStart::Join`, which takes the place of
  `HistoryStart::After`); `HistoryAccess::None` and `LastMessages { count: 0 }` read no page to
  find that out. `ChannelLoop::history_start` hands it to the actor's start as `None` - what is
  said from its start on, as for a founder -, so that what `run` records between taking someone
  in and their join is not shared either, as it would be from a position fixed before. All of it
  became a variant too (`HistoryStart::Beginning`), so that `history_start` is one `match`;
  `HistoryStart::message` and `from`, which only it used, went. Shares already recorded so stay
  in the histories, and the ranges they began in the stores, until the stored histories are
  deleted.
- **2026-11-15** — Made consistent, as the maintainer decided: a grant counts and dates messages
  alone, and joins and similar events are none. Granted as many messages as there were or more, or
  a time span all of whose messages were recent, a newcomer was given all of the history - the
  joins and other events before its oldest message too -, while one granted fewer was given them
  only from a message on. The view now begins at the oldest message granted, at the join where
  none is; only `HistoryAccess::All` begins at the history's start (`HistoryStart::Beginning`).
  `LastMessages { count: 0 }` is no case of its own any more: no message granted. Shares recorded
  so from the history's start stay in the histories, and the ranges they began in the stores,
  until the stored histories are deleted.
