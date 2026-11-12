# 564 — Taking someone into a session and a channel is rolled back in part only

**Status:** open
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
