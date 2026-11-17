# 590 — A session lets nobody go

**Status:** in progress
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

## Plan

Decided with the maintainer on 2026-11-17. The pattern is the channel's: `remove_participant`
and `leave` by one way out (`ChannelLoop::let_go`, `Exit`), `ChannelController::remove_participant`
announcing also to the one let go, `Channel::let_go`. What it keeps: whoever a channel lists takes
part in its session - what issue 564 rests on.

```rust
// caelumora-domain
impl Session {
    /// Lets `actor` go: out of its participants; the participant, or `None` if it took no part.
    pub fn let_go(&mut self, actor: ActorId) -> Option<Participant>;
}
pub enum SessionUpdateEvent { …, ParticipantLeft(ActorId) }

// caelumora-security
ActionType::LeaveSession(&SecurityAttributes)   // rule 3, as LeaveChannel: always allowed

// caelumora-kernel
impl SessionLoop {
    /// Lets `participant` go in the name of `context`, which may change the session: out of
    /// the session, then out of every channel of it.
    pub async fn remove_participant(&self, context: &SecurityContext, participant: ActorId)
        -> Result<()>;
    /// Takes whom `context` acts for, or their own agent `participant`, out of the session
    /// by their own act.
    pub async fn leave(&self, context: &SecurityContext, participant: ActorId) -> Result<()>;
}
impl SessionController {
    /// Lets `participant` go; the group follows; announced to the one let go too.
    pub(crate) async fn remove_participant(&self, ctx: &SecurityContext, participant: ActorId)
        -> StoreResult<Participant>;
}
```

A sub-agent of 558 is its main agent's owner's, and the main agent acts for that owner, so its
work lets the sub-agent go by `leave` (`Participation::require_own_act`): no third way, in the
kernel's name, is needed.

Decided:

- **F1 - Channels that do not run.** The operation is the running session's - the REST server
  opens a session first -, but a session holds channels that do not run: one whose components
  cannot be read (`ChannelEntry::Unavailable`, held with its document as it was), and its deleted
  ones, kept in the store. Either would start the one let go again as it is built. Their stored
  documents are changed as well, and the copy held of an unavailable one is renewed, so that the
  rights by the group of a channel's participants go at once too. Leaving them out as a channel is
  built would bring one back into a channel they had been let go from, once taken into the
  session again.
- **F2 - The order.** The session first, then its channels: once the session is stored, taking
  the participant into a channel is refused (`NotInSession`), and one under way holds the lock of
  its participant in its channel, which letting go waits for, and finds them listed. A channel
  that fails afterwards - a store's failure alone - leaves the error to the caller, and asking
  again finishes letting go; it is not rolled back, since taking someone out of a channel ended
  their turn. Named as an exception in the rule of rolling back.
- **F3 - In whose name the channels change**: the kernel's, once letting go from the session was
  permitted, as founding and joining store; each channel keeps who let them go (`Removal::by`). A
  way out of the channel of its own, which asks nothing (`Exit`, as `Entry::Founding`).
- **F4** - no former participants of the session: its channels keep theirs, and nothing would read
  the session's. No rule for its owner or its last participant, as for a channel.
- **F5 - Users.** The kernel's operations and `LeaveSession` first, which 557's step 2b needs;
  the REST routes (`DELETE /sessions/{id}/participants/{aid}`, `POST …/{aid}/leave`) and the
  web UI's sidebar, which keeps a session it no longer reads, as a step of their own. Commands
  once someone needs them.
- **The exception of 564** goes: one the channel cannot take in after the session took them in is
  rolled back out of the session, their invitation given back (`Session::undo_take_in`, as the
  channel's), as a step of its own, the last.

Steps, each committed on its own:

1. `Session::let_go`, `SessionUpdateEvent::ParticipantLeft` - taken over again harmlessly, as a
   controller's own announcement comes back to it -, `SessionController::remove_participant`.
2. `SessionLoop::remove_participant` and `leave`, through every channel of the session, running
   or not; `LeaveSession`; the security suite; CHANNELS.md, PERMISSION-CHECKS.md, SECURITY.md, and
   the sentence "a session lets nobody go" where it stands.
3. The REST routes and the web UI; `SessionError::NotAParticipant`, answered as `invalid_channel`
   today, answered for a session.
4. The exception of 564 rolled back.

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
- **2026-11-17** — Planned with the maintainer (*Plan*): the session first, then every channel of
  it, also those that do not run, in the kernel's name; leaving by one's own act under
  `LeaveSession`; no former participants of the session; the REST routes and the web UI as a step
  of their own; the exception of 564 rolled back. The consideration that a stopped channel is
  changed by `Channel::let_go` did not hold: nothing changes a channel that does not run today.
  Status: in progress.
- **2026-11-17** — Steps 1 and 2 done together: step 1 alone would have left
  `SessionController::remove_participant` without a caller. `Session::let_go`,
  `SessionUpdateEvent::ParticipantLeft`, `SessionController::remove_participant` - announced to the
  one let go as well -, `ActionType::LeaveSession`, `SessionLoop::remove_participant` and `leave`.
  The session is stored first, then every channel of it in the kernel's name: a running one by a
  way out of its own (`Exit::WithTheSession`), one that does not run in its stored document and
  the copy the session holds (`let_go_where_unavailable`), never while it is built again
  (`SessionLoop::rebuilding`, which `retry_unavailable` takes as well); a deleted one, whose
  document the store does not change, lets go whoever is not in the session as it is restored,
  removed by the kernel, without their clearance asked - decided with the maintainer. Asking again
  finishes a letting go a channel failed. CHANNELS.md (*Letting someone go from the session*),
  PERMISSION-CHECKS.md, SECURITY.md and the rule of rolling back, which names letting go as an
  exception. Left open, for the maintainer: a channel that does not run records no leave in its
  history, so the participant's range of attendance there stays open; a restoring under way while
  someone is let go brings them back with the channel; and a session takes over its own
  announcements on top of its live copy, so that a join handled after a leave lists the one let
  go again for a moment.
