# 420 — A channel is built empty, and its participants join by the one way in

**Status:** done
**Type:** refactoring
**Priority:** medium
**Area:** Sessions and channels
**Summary:** A channel's founders are written into its document as it is built and never join, so no `ParticipantJoined` records them; build it empty and let its builder add them as anyone is added.

## The problem

There are two ways into a channel. `Channel::build` takes the founders' list and writes it into the
document; `SessionChannels::create_channel` hands it what the creator asked for. Everyone added
later goes through `ChannelController::add_participant`, and as their actor starts, `run` records
`HistoryShared` and `ParticipantJoined` in the adder's name (issues 408, 417).

The founders never join: nothing in the channel's history says they are members, nor since when.
Issue 417, step 7, reads membership from the chain alone - every member's `ParticipantJoined`,
the end of membership from its `ParticipantLeft` - and derives each member's visible ranges from
it. A founder would read nothing.

Two ways in are also two places where admission is checked, the group of the participants is kept
(`keep_participants`) and a newcomer is announced.

The approver makes a third case. `Channel::build` adds one to the participants if none is named, a
template may name one, and from then on nearly every reader of the participants takes it out again:
`Group::members_among`, the workspace reaper, `remove_participant` (`ApproverStays`), and the web UI
in five places - it is not introduced, not offered, not removable. It is no participant of the
session either: `SessionLoop::add_participant` refuses it. It cannot join by the one way in, since
there is nothing to take it in from, and it has nothing to read in the history: approvals are not
recorded there (issue 417, decision 3).

## Decision

Two steps, each reviewable on its own.

1. **The approver is no participant: it is the first of the channel's internal actors.** An
   internal actor belongs to the channel, as its moderator does: it stands in
   `Channel::internal_actors` (`InternalActor`, for now only `Approver` with its id and strategy),
   not among the participants, nor in their group; it cannot be removed, never joins nor leaves,
   and has no range of the history - what is recorded to it goes to it by name (`Only`). Otherwise
   it is an actor like any other: an `ApproverActor` with its inbox and task, sending through
   `run`'s transport, registered with the moderator, `ActorId::Approver` as the sender of what it
   decides; what reaches it is decided by the one rule (`Envelope::concerns`). The channel starts
   it with the others as `run` begins. Internal actors are configured, and what is not configured
   is not there: a channel template names them for each channel (`internal_actors`), and so does
   a request to create a channel; `Channel::build` takes them as given, with no fallback and no
   check of how many approvers there are. A channel without an approver approves nothing - tools
   that ask for approval do not work in it. `Participant` is a user or an agent. What falls away:
   `ApproverStays`, the approver's branch of `Participation::resolve`, the filters in
   `Group::members_among`, the reaper and the web UI. Over REST, the approver leaves the
   participants' list. An internal actor as a component of the `ComponentRegistry`, which a
   plugin could contribute, is left until a second kind asks for it.
2. **A channel is built empty, and its founders join by the one way in.** `Channel::build` takes
   no participants. `create_channel` keeps the order of issue 304 - its verb, then every founder's
   admission, before anything is stored -, stores the empty channel, builds its loop and takes
   each founder in by `ChannelLoop::take_in`, which then knows three ways in: added, joining, and
   founding. Whatever fails after the channel was stored rolls its creation back
   (`roll_back_channel_creation`). A session made from a template takes in its users as it takes
   in its agents today (`take_in_agents`), in the same way. As `run` begins, it first applies the
   changes that wait - the founders taken in before it ran -, so that their joins are recorded
   before anything an actor sends is routed: today an actor started at the beginning may speak
   before the join of another one, which received it, is recorded. A channel restarted or restored
   records no join: its members' joins are in its chain.

Decided as well:

- **What founding asks.** No verb of its own: creating the channel (`CreateChannel`) is what lets
  its creator name its founders, as today. Every founder's admission is asked as for anyone taken
  in, and they are stored in the runtime's name, as a join is. Founding is reachable from
  creating a channel alone (`create_channel`, `establish`), or it would pass by `UpdateChannel`.
  The security suite checks that a creator without `UpdateChannel` founds a channel with others,
  and that a founder refused leaves no channel.
- **`HistoryShared` only where something is shared**: recorded only if the range it grants begins
  before the join. A founder, and whoever is given nothing (`HistoryAccess::None`), has none. A
  member's visible range begins at the `from` of its `HistoryShared`, otherwise at its join. Sent
  in the system's name, with `by` in the entry (issue 417, step 7).
- **On a branch of its own**, from issue 417's, merged into it once reviewed: it changes the way
  in that 417's steps 5 and 6 changed, and 417's step 7 builds on it.

## History

- 2026-10-18: Opened from issue 417, step 7, which needs every member's `ParticipantJoined`.
- 2026-10-18: The approver taken out of the participants as step 1; the design of step 2 laid out.
- 2026-10-18: Step 1 as internal actors, the approver the first; what founding asks, where
  `HistoryShared` is recorded and the branch decided.
- 2026-10-18: Step 1 done on `issue-420-channel-built-empty`. `Participant` is a user or an agent;
  `Channel::internal_actors` holds the channel's approver (`InternalActor::Approver`), which
  `Channel::build` makes deciding by `Any` and a template by its channel's `approval`. The
  `ChannelLoop` builds the internal actors' actors from the channel itself
  (`ResolvedParticipant::internal`) and adopts them as before, so the approver's actor, its
  delivery and the moderator's filing of it are unchanged. Gone: `ApproverStays` - removing the
  approver is `NotAParticipant`, and its actor runs on -, the approver's branch of
  `Participation::resolve`, `ApproverParticipant` in the views, the filters in
  `Group::members_among`, the workspace reaper, the agents' participants prompt and the web UI,
  `ParticipantTemplate::Approver` and the REST `ParticipantKind::Approver`. Over REST, an approver
  is in no participants' list any more. The seed's templates name no approver.
- 2026-10-18: Internal actors configured, no fallback: `ChannelTemplate::internal_actors` and
  `ChannelSpec::internal_actors` (`InternalActorTemplate`, an approver with its strategy) name them,
  `Channel::build` makes none; `ChannelTemplate::approval` is gone. The `ChannelLoop` keeps all its
  actors in one map, as before; who takes part (`participants`, `participant_ids`) and whom a
  mention names leave out what the channel configures as internal (`Channel::is_internal`), so
  that `is_mentionable`, which told the approver by its kind, is gone. Over REST, creating a
  channel takes `internal_actors`, none if absent; the web UI's dialog names an approver. The
  seed's channels name theirs. The test worlds give their channels an approver, as `build` did.
- 2026-10-18: Step 2 done; the issue is done. `Channel::build` takes no participants: a channel is
  built and stored empty, and its founders join it by `ChannelLoop::found` - `take_in` with a third
  way in besides added and joining (`Entry::Founding`), which asks no verb of its own and stores in
  the runtime's name. `SessionLoop::create_channel` asks the founders' admission before anything
  is stored (`Participation::require_admitted` now takes them), founds them once the channel runs,
  takes the channel back if one cannot join (`abandon_channel`), and announces it as created once
  they joined. A template's channels are built empty too (`ChannelTemplate::instantiate` checks
  the names alone); its users and agents join the channels that name them as founders once the
  session is made (`take_in_participants`, formerly `take_in_agents`), each agent taken into the
  session first; `establish` asks no admission of empty channels, and the `SessionManager` keeps
  no `Participation`. In tests, `SessionManager::create` founds the participants its channels
  name. `run` applies the changes that wait before it routes anything. Security suites: creating
  a channel asks each founder's clearance again as they join and no `UpdateChannel`; a template's
  founders ask neither `GetChannel` nor `UpdateChannel`; new, a founder who is not cleared leaves
  no channel. Founders' `HistoryShared` grants from the start; recording it only where something
  is shared is issue 417's step 7.
