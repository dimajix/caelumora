# 590 — A session lets nobody go

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** Nobody can be taken out of a session: one taken out of each of its channels, or a sub-agent whose work is done, stays among its participants and in the group of its participants - reading the session, writing into a workspace made for it, reading its own project.

## The problem

A participant comes into a session (`Session::take_in`, issue
[221](221-adding-session-participants.md)), but nothing takes them out again. A channel lets its
participants go (`Channel::let_go`, kept as former participants, issue
[164](164-soft-deletion-and-former-members.md)); the session has no counterpart. Issue 221 left it
out ("Removing someone from a session ... is not part of this"), issue 164 noted that nothing
removes participants from sessions yet, and issue
[564](564-taking-someone-in-is-rolled-back-in-part.md) took the gap as a reason: one the channel
cannot take in after the session took them in stays in the session, since rolling them back would
need a way out of a session for this case alone. Since then CHANNELS.md and the rule of rolling
back (CODING-GUIDELINES.md) say "a session lets nobody go" as if it were a rule; it was never
decided.

What being in a session gives stays with whoever was ever in it: the group of its participants
(`Group::of_session`, which follows `Session::participants`) reads the session
(`Session::attributes_of`), writes into a workspace made in its scope (`Workspace`), and reads its
own project (`Project::own_of`).

## Why it matters

- **Sub-agents** (issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md)) are taken into a
  channel for one task and end with it. Decided there: a sub-agent whose work is done disappears
  from the participants of its channel and of its session, and is deleted - not purged: what it
  did, its document and its state are kept for tracing what happened. Without a way out of the
  session, every sub-agent ever started stays listed in it, with its rights.
- **One taken out of every channel** still reads the session, writes into its workspace and reads
  its project, though nobody meant them to take part any more.

## Considerations

- **What letting go does.** Out of every channel of the session first - a running one as
  `ChannelLoop::remove_participant` does, ending the actor's turn as a removal does; a stopped one
  in its document (`Channel::let_go`) -, then out of `Session::participants`; the group of its
  participants follows as the session is stored (`keep_participants`), and the change is announced
  as `SessionUpdateEvent::ParticipantLeft`, the counterpart of `ParticipantJoined`, so that views
  and clients follow. Nothing is deleted: what they said stays in the channels' histories, the
  channels keep them as former participants. Deleting a sub-agent is the next step of 558, not part
  of letting go.
- **Who may.** Whoever may change the session (`UpdateSession`); by their own act, oneself or
  one's own agent, as leaving a channel is (`ChannelLoop::leave`). A sub-agent is let go by what
  ends it in 558 - in whose name is to be decided there.
- **Coming back.** A personal invitation was used up as they came in (`Session::take_in`), so
  coming back takes an entry for a role, a group or everyone, or whoever may change the session -
  as for a channel.
- **Former participants of the session.** The channels keep theirs, with the removal; whether the
  session keeps its own as well - for a view of who was in it - or the channels' are enough.
- **Deleted agents.** A deleted agent rests in its channels and comes back when restored (issue
  164); deleting is not letting go, and stays so. A sub-agent is let go first, then deleted, so
  that restoring it brings it back into nothing.
- **Open: the exception of 564.** With a way out of a session, the reason given for not rolling
  back the session's part - one the channel cannot take in after the session took them in - falls
  away. Whether to roll it back now, or keep the state "in the session and in none of its channels"
  as one of its own, as decided there. Either way, the sentence "a session lets nobody go" in
  CHANNELS.md and CODING-GUIDELINES.md goes.

## Done when

- A participant can be let go from a session - by whoever may change it, or by their own act for
  themselves or their own agent -: out of every channel of it, running or stopped, then out of the
  session and the group of its participants, announced; covered by tests and the security suite.
- Whether the session keeps former participants is decided, and done.
- The exception of 564 is decided - rolled back or kept - and CHANNELS.md and CODING-GUIDELINES.md
  say so instead of "a session lets nobody go".

## History

- **2026-11-17** — Opened from the discussion of how a sub-agent of issue 558 ends: that a session
  lets nobody go turned out to be a gap, not a decision. Decided with the maintainer: a sub-agent
  whose work is done disappears from the participants of its channel and session and is deleted,
  never purged - its data stays for tracing what happened. Whether the exception of 564 goes with
  this is left open.
