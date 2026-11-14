# 564 — Taking someone into a session and a channel is rolled back in part only

**Status:** done
**Type:** bug
**Priority:** low
**Area:** Sessions and channels
**Summary:** Taking someone into a session together with a channel stores the session first and keeps it when the channel fails, two takings-in of one participant can race and undo each other, and a join is announced for one already listed; with them, a few loose ends of the same code - visibility, a rollback step not named so, an invitation built by hand.

## The problem

Issue [563](563-a-channel-invitation-is-never-used-up.md) made a channel use up an invitation
as the session does, and rolled back an addition to a channel whose actor cannot start as the
channel had the participant before (`ChannelController::roll_back_participant_addition`,
`Channel::undo_take_in`). Its reviews found what lies beside it:

1. **The session's part is not rolled back.** `SessionLoop::add_participant` and
   `SessionLoop::join` (`session/participants.rs`) take the person into the session first -
   joining, which uses up their invitation to the session (`Session::take_in`) - and then into the
   channel. If the channel refuses or fails after that - an actor that cannot start,
   `StillLeaving`, a clearance -, the person stays in the session and the invitation to it stays
   used up: stored part of the way, against
   [Rolling back](../docs/CODING-GUIDELINES.md#rolling-back). CHANNELS.md says "The session
   first, then the channel. One the channel refuses after all stays in the session"
   ([CHANNELS.md](../docs/CHANNELS.md#taking-someone-into-the-session)); whether that is meant
   for a failure as well as for a refusal is to be decided with it.
2. **Two takings-in of one participant race.** `ChannelLoop::take_in` refuses one whose actor
   runs (`AlreadyParticipant`), but two calls for the same participant can both pass that check
   before either has started an actor. If the first then fails, its rollback takes the participant
   out of the document while the second one's actor runs, unlisted. It was so before 563 as well,
   with `remove_participant` in place of the rollback.
3. **A join is announced for one listed already.** `ChannelController::add_participant`
   announces `channel::JOINED` whether or not the participant was listed before - a listed agent
   whose actor never started can be added again, since `take_in` checks running actors, not the
   document. The rollback of 563 announces a leave only where the listing changed; the addition
   does not do the same.
4. **`ChannelController` is more visible than it is used.** Only the kernel uses it, yet its
   operations - `add_participant`, `remove_participant`, `update_*` - are `pub`.
5. **A rollback step not named so.** `SessionSettings::take_back_entering`
   (`session/settings.rs`) rolls back what `update_project` did before storing the session failed,
   but is not named `roll_back_…` as the rule of rolling back names such steps, and "take back" is
   already the word for retracting a message ([CHANNELS.md](../docs/CHANNELS.md)).
6. **An invitation built by hand.** The test of issue 221
   (`whom_the_access_list_lets_join_takes_themselves_in_and_uses_up_an_invitation`,
   `session/participants.rs`) builds `AclEntry { subject: Subject::Actor(…), grant: Grant::Join }`,
   which has a name since 563: `AclEntry::invitation`.

## Considerations

- **Rolling back the session's part** (1): the session's controller would answer the session as
  it was, as the channel's does, and a `Session::undo_take_in` set back what `Session::take_in`
  changed - out again unless in it before, the invitation given back -, unless they were taken out
  meanwhile; announced as a leave. Or the channel is asked everything it can refuse before the
  session is stored, leaving only failures to roll back.
- **Serialising takings-in** (2): `take_in` held per participant - a set of those being taken in,
  refused as `AlreadyParticipant` while one is - or the check made against the document under the
  controller's write lock.
- **Announcing a change only** (3): `add_participant` announces a join only where the
  participant was not listed, as the rollback does.
- 4 to 6 change no behaviour and can go with any of the above.

## Done when

- One the channel cannot take in after the session took them in is rolled back as decided, the
  invitation to the session included; the security suite covers it.
- Two takings-in of one participant cannot leave a running actor unlisted.
- A join and a leave are announced only where who is listed changed.
- `ChannelController`'s operations are `pub(crate)`, `take_back_entering` is named by the rule,
  and the test of 221 uses `AclEntry::invitation`.

## History

- **2026-11-12** — Opened from the findings of the reviews of issue
  [563](563-a-channel-invitation-is-never-used-up.md), gathered in one issue as the maintainer
  asked.
- **2026-11-14** — Decided with the maintainer: what the channel can refuse is asked before the
  session is stored already (`SessionLoop::take_in`), and one who is not in the session has never
  been in one of its channels - a session lets nobody go -, so after the session was stored only a
  failure or a race is left. One the channel then cannot take in stays in the session, as one
  taken in without a channel does, and can be taken into the channel again; the session's part is
  not rolled back, which would need a way out of a session for this case alone. This is
  documented as a deliberate exception and held by a test. Besides, a join is announced only
  where who is listed changed - in the session too, which covers two joins of one person racing
  there -, and takings-in of one participant into a channel are serialised in its `ChannelLoop`.
  Four steps: the changes without behaviour (4 to 6), the announcements, the serialisation, the
  exception.

  Step 1 done: `ChannelController` is the kernel's (`pub(crate)`, the type, its operations and
  `ControllerManager::new_channel_controller`), and so is `ChannelRuntime::new`, which takes it;
  `take_back_entering` is `roll_back_project_change`; the test of 221 gives and checks the
  invitation as `AclEntry::invitation`.

  Step 2 done: `ChannelController::add_participant` announces `channel::JOINED` only where the
  participant was not listed before, and `SessionController::add_participant` announces
  `ParticipantJoined` only for one who did not take part before - both judged on the document as
  the change is stored, under the controller's lock, so that two who take one person in at once
  announce them once. In the channel itself the join is still told and recorded for one added
  while listed already: its actor starts only now.

  Found on the way, still open: `ChannelLoop::follow_agents` starts the actor of a listed agent
  that has none - as the session starts, and once a deleted agent is restored - beside `take_in`,
  and can race it as two takings-in do. It announces a join (`announce_resting`) also for an agent
  that was left out as its channel started, for which no leave was ever announced.

  Step 3 done: a `ChannelLoop` makes the changes of one participant one at a time, by a lock per
  participant (`LocksById`, as the managers hold one per entity): `take_in`, `let_go` and
  `follow_agents` hold it while they change the participant's part, and `follow_agents` looks
  again under it whether the agent is still listed. Two who add one participant at once are served
  one after the other - the second finds them there -, a rollback cannot take out one whose actor
  another started, nor a removal leave a participant's actor running unlisted, and `follow_agents`
  no longer races a taking-in or a removal. Only the first is held by a test: the window of the
  others is not reached by running them at once. That `follow_agents` announces a join for an agent
  left out as its channel started stays open.

  Decided with the maintainer: the join `follow_agents` announces for an agent left out as its
  channel started stays, described at `follow_agents` - rare, harmless to who hears it, and telling
  it from one that rested would need the channel to keep why it left out whom. And, as a step of
  its own before step 4, without behaviour: the document answers whether it listed or unlisted.

  Step 3b done: `Channel::take_in` and `Session::take_in` answer whether they listed the participant
  anew, `Channel::undo_take_in` whether it took them out, as `Channel::let_go` answers whom it let
  go. The controllers announce by these answers instead of judging the listing again;
  `SessionController::add_participant` takes the participant in by `Session::take_in` as the
  channel's controller does by `Channel::take_in`, and `Session::apply` takes over an announced
  join without the answer.

  Found by the review of step 3b, for the maintainer: a join recorded in a channel's history for a
  participant whose range of attendance is still open - one added while listed already (step 2),
  the join of `follow_agents` above, an agent deleted while its session was stopped and restored
  later - opens a second range (`Attendance::take`), and a later leave closes only the last one. The
  first stays open, and what the participant is shown of the history (`Attendance::of_reader`)
  then reaches past their leave. Opened as issue [568](568-a-second-join-leaves-attendance-open.md).

  Step 4 done: one the channel cannot take in after the session took them in stays in the session,
  as decided - stated in CHANNELS.md, named as the exception in the rule of rolling back
  (CODING-GUIDELINES.md), and at `SessionLoop::add_participant` and `join`. The security suite
  holds it: an agent brought in by its owner's invitations, which cannot start, stays in the
  session and out of the channel, and tries again by the channel's entry alone. That an invitation
  to the session stays used up is not held by a test: an agent is invited by its owner, whose
  invitation stays, and a user's actor has no way to fail as it starts that a test reaches.
