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
3. A channel reconciles its history as it is built: whoever it keeps as a former participant with
   a range of attendance still open in it - let go while it did not run - is recorded as left, in
   the kernel's name, ahead of every actor it starts. The channel stays the one writer of its
   history; nothing was recorded while it did not run, so the leave stands where it would have
   stood. A participant without an actor removed from a running channel - left out as it started -
   is recorded as left at once. The pattern is documented (below).
4. A session takes over a change made elsewhere by reading its document as stored, as the
   controllers of projects and workspaces do (`take_stored`), not by applying its own
   announcements again on top of its live copy (`LiveDocument::take_over`): a join handled after
   the leave that followed it lists the one let go again for a moment, and a change of the
   session meanwhile would store them back - what a sub-agent of 558, taken in and let go soon
   after, runs into.
4b. A session follows whom it takes in where it takes them in, not by hearing its own
   announcement on the bus: it subscribes to its own topic for this alone since step 4, and every
   event of its channels passes its loop to be dropped; a stale or forged join would have it follow
   one let go. Its subscription then no longer lives on `run`'s stack alone.
5. A history store answers who has a range of attendance open in a channel at once, so that a
   channel being built reads them in one question instead of one per former participant
   (`HistoryStore::attendance`, step 3), whose number grows with every sub-agent.
6a. An actor is named in a path of the REST API by its kind and its id, not by a bare UUID (below):
   before step 6, so that the routes of a session's participants do not need a lookup of the
   kernel's that turns a UUID back into an actor. Its behaviour changes where the REST server's
   own check of participation departed from the kernel's.
6. The REST routes and the web UI; `SessionError::NotAParticipant`, answered as `invalid_channel`
   today, answered for a session.
6b. Whether one may leave a session is the session view's to say, as a channel's participants
   carry `can_leave` (`ParticipantView::may_leave`): the web UI offers to leave a session by a rule
   of its own today - whether its participants list the user.
7. The exception of 564 rolled back.
8. A session makes the changes of one participant one at a time, by a lock per participant
   (`LocksById`, as a channel's `changing`), which taking in - the session, then the channel, and
   rolling back -, and letting go hold: two takings-in of one person at once, the first failing in
   its channel, cannot roll back one the other took into a channel meanwhile, nor letting go and
   taking in race. The lock order is the session's, then the channel's, everywhere.

**Step 6a, actors in paths** (agreed): the routes of a channel's participants and actors
(`…/participants/{aid}`, `…/actors/{aid}`, and the private notes of `/sessions/{id}/actors/{aid}`)
take a bare UUID, which the REST server looks up among the channel's actors (`actor_in`) or
participants (`participant_of`) to learn which kind of actor it is - two tasks in one, finding the
kind and checking that they take part, of which the second is the kernel's and the views', which
ask it anyway. Proposed: a path names an actor as `User:<uuid>` or `Agent:<uuid>` - the kind as the
JSON of an `ActorId` names it, what the web UI keys an actor by already (`actorKey`) -, read by a
type of the REST API:

```rust
// caelumora-rest-api
/// An actor as a path of the REST API names it: its kind and its id, `User:<uuid>` or
/// `Agent:<uuid>`.
pub struct ActorPath(pub ActorId);
impl FromStr for ActorPath { … }        // a user or an agent; anything else is refused
impl fmt::Display for ActorPath { … }   // the same form
impl<'de> Deserialize<'de> for ActorPath { … }   // by `FromStr`, for a path's extractor
```

The handlers hand the `ActorId` to the kernel and the views as it is; `actor_in` and
`participant_of` go, the view answering a participant itself (`ChannelView::participant`, which
already knows a former one). The web UI addresses an actor by `actorKey`; `actorUuid` goes.

**Reconciling as a pattern** (the maintainer, 2026-11-17): a running object is the one writer of
what it holds. What changes while it does not run is stored in its document alone - by a
controller of its own, in the kernel's name -, and as it is put into service again - a session, a
channel, a thread - it first brings what it derives from the document in line with it, as though
the change had happened while it ran: its history, its actors - the groups of its participants
follow as the document is stored. What is lost is the order of changes among each other and their
exact time where the document does not keep them; a queue of the acts themselves was weighed and
not taken - more than this needs. The session's `follow_agents` as it starts already does so for
agents deleted or restored meanwhile.

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
- **2026-11-17** — Decided with the maintainer: the history of a channel that did not run is
  reconciled as it starts (step 3, before the REST routes), and reconciling as an entity is put
  into service again is a pattern of its own (*Plan*). The session writes no history: the
  `HistoryStore` is the read side, and a channel's history is written with its state by its loop
  alone.
- **2026-11-17** — Step 3 done: `ChannelLoop::reconcile`, as the channel is built, hands `run` the
  leave of each former participant whose attendance is still open (`attending`, beside
  `rejoining_from` in `channel/history.rs`), ahead of every actor it starts, so that `run` stays
  the one writer of the history and nothing can be removed or taken in again before it; a
  participant without an actor removed from a running channel is recorded as left at once. The
  leave is recorded in the kernel's name at the time of the start, not with who let them go and
  when, which the document keeps. `ActorChange::Leave` carries the name as said. The pattern is a
  principle in ARCHITECTURE.md (*Reconciled as it starts*). Left for the maintainer: each start
  reads the attendance of every former participant one by one, which a store method answering
  the open ranges of a channel at once would replace; and one whose actor had not ended as its
  session stopped has its attendance end only as the channel is built again.
- **2026-11-17** — Decided with the maintainer: the two points left by step 3 are steps of their
  own, 4 and 5 (*Plan*), before the REST routes.
- **2026-11-17** — Step 4 done, simpler than planned: the session takes over its own
  announcements no more at all, rather than reading itself again as stored. Its controller is the
  one writer of it in its instance and holds every change live already - as EVENTS.md and
  STORAGE.md said, while `SessionLoop::handle_global_event` still applied each announcement on top
  of the live copy; `SessionController::process_global_event` is gone. The session follows its own
  announcements only for whom it takes in. A test holds that a join heard of after the leave that
  followed it lists nobody again.
- **2026-11-17** — Accepted by the maintainer: step 4 as done, without `take_stored`. The finding
  of its review - the session hears its own joins through the bus only to follow whom it took in -
  is a step of its own, 4b (*Plan*).
- **2026-11-17** — Step 4b done, as the maintainer agreed the interface: a subscription is followed
  by its `Following` (`EventSubscription::following`), which `EventBus::follow` takes in place of
  the subscription. A session subscribes as it is made, to the topics of its participants alone,
  and keeps its `Following`; `run` takes the subscription over. Whom it takes in it follows in
  `take_in`, before storing them, so that nothing of them is missed meanwhile - one whose storing
  fails is followed for nothing, which is harmless. It hears none of its own announcements and
  none of its channels' events any more; `participant_taken_in` is gone. Unlike the agreed
  interface, it no longer follows its participants again after lost events: a loss drops events,
  not topics, and whom it takes in it follows as it takes them in, so that step had nothing left to
  do (`participant_topics` went with it). `Following`, `EventSubscription::following` and
  `EventBus::follow` are the kernel's (`pub(crate)`). The tests of a loss flood a topic the session
  still follows; the one of an agent taken in while events were lost is gone, its case with it.
- **2026-11-17** — Step 5 done, the interface agreed with the maintainer: `HistoryStore::attending`
  answers who takes part in a channel's history now, each with the thread of their open range,
  from the attendance every store reads whole anyway (`Attendance::attending`); in the memory, the
  JSON and the Postgres store, held by the store suite. A channel reads it once as it is built
  (`reconcile`) and as it removes one without an actor; `ChannelLoop::attending` answers all of
  them, no longer one. Postgres not tested here, without a database.
- **2026-11-17** — Decided with the maintainer: before the routes of step 6, an actor is named in a
  REST path by its kind and id (step 6a, *Plan*), so that no lookup of a UUID - finding the kind
  and checking participation at once - is needed. The bare UUIDs of events, correlations and spans
  are issue [592](592-event-span-and-correlation-ids-are-bare-uuids.md).
- **2026-11-17** — Step 6a done, as proposed and agreed: `ActorPath` in `caelumora-rest-api` reads
  and writes `User:<uuid>` and `Agent:<uuid>`; the routes of a channel's participants and actors
  and the private notes of a session's agent take it; `actor_in` and `participant_of` are gone, the
  handlers handing the actor to the kernel and to `ChannelView::participant`. The web UI names an
  actor by `actorPath` (its `actorKey`) in place of `actorUuid`. Where the REST server checked
  participation itself - among the running actors alone - and the kernel checks it otherwise, the
  kernel's answer stands now: asking an actor that takes no part to answer again is answered `200`
  and refused on the stream (`EventRejected`), not `404`; removing a participant listed without an
  actor - a deleted agent resting, one left out as the channel started - removes it (`204`), as
  issue 314 meant, where it was `404`, and so does its owner's leaving of an agent left out (a
  resting one is refused, `403`); cancelling an actor's turn or a tool call asks `CancelChannel`
  first, so that one who may not is answered `403` for an actor that takes no part as well. A user's
  private notes are none, as before.
- **2026-11-17** — Step 6 done: `DELETE /sessions/{id}/participants/{aid}` and
  `POST /sessions/{id}/participants/{aid}/leave`, the actor named by its `ActorPath`;
  `SessionError::NotAParticipant` is `404 participant_not_found`, also where a channel is created
  with founders not in the session (`400 invalid_channel` before). The web UI's session menu leaves
  a session, for one who takes part, asked once more, and the list drops a session it fetches again
  and the user no longer reads - the owner's stays. Removing someone else from a session has no
  dialog yet: the web UI manages no session's participants. Not seen in a browser.
- **2026-11-17** — Decided with the maintainer: whether one may leave a session is told by the
  session view, as for a channel (step 6b, *Plan*), from the finding of the review of step 6.
- **2026-11-17** — Step 6b done, the interface agreed: a session's participant as the viewer sees
  it carries `can_leave` (`ParticipantSnapshot`, `ParticipantResponse`) - themselves, or an agent
  of theirs that exists -, decided by the session view as a channel's is, the policy not asked;
  the web UI's session menu offers to leave by it. The views read an actor's name and whom it
  answers to from one read (`Views::name_and_owner_of`), which `name_of` answers by.
- **2026-11-17** — Step 7 done: one the channel cannot take in after the session took them in is
  rolled back out of the session as it had them before (`Session::undo_take_in`,
  `SessionController::roll_back_participant_addition`, as the channel's), in the kernel's name,
  their invitation given back, a leave announced where who is listed changed; by
  `SessionLoop::add_participant` and `join` (`roll_back_taking_in`). The exception of 564 is gone
  from CHANNELS.md, the rule of rolling back and PERMISSION-CHECKS.md; the security suite holds it
  for joining, a session test for taking in, with the group and the announcement. Giving an
  invitation back is `SecurityAttributes::give_back_invitation`, which both `undo_take_in`s use.
  That the invitation of one who joined themselves is given back is held by the domain's test
  alone: a user's actor has no way to fail as it starts that a test reaches, as 564 found. Left
  for the maintainer: the session holds no lock per participant, so one taken into another channel
  between the session's storing and its rollback - two takings-in of one person at once, the first
  failing in its channel - is listed there and out of the session; letting go from the session
  finishes it.
- **2026-11-17** — Decided with the maintainer: a lock per participant in the session, as a step of
  its own (step 8, *Plan*), from the finding of the review of step 7.
