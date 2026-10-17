# 120 — Sessions and channels store copies of their participants

**Status:** done
**Area:** Sessions and channels
**Summary:** Sessions and channels store full copies of their participants' user records, which go stale and disclose email and roles to every participant.

## The problem

`Participant` (`caelumora-model/src/channel.rs:133`) is `User(User)`, `Agent(Agent)` or
`Approver(Approver)`: the whole record, not a reference. `ParticipantTemplate::instantiate`
(`template.rs`) reads the user from the store and puts the copy into `Session.participants` and
`Channel.participants`, which are stored with the session and the channel.
`ChannelLoop::create_actor` then builds the `UserActor` - and with it the `UserController` - from
that copy, not from the store.

Agents are embedded the same way.

## Why it matters

- **The copy goes stale.** A renamed user keeps the old name in every session made before, and after
  a restart the `UserController` starts from that old name again; `UserUpdated` only reaches
  controllers that happen to be running.
- **It discloses more than the session needs.** A user record belongs to the system and holds
  `email` and `roles` (issue [94](94-users-unchecked-in-store.md)); every participant who may read
  the session reads these of every other participant, and they end up in every session document and
  REST response.
- **Two sources of truth.** Whoever changes a user through the store changes one of them.

## Considerations

- **Reference, resolve at load (the maintainer's direction).** A participant holds only the id
  (`Participant::User(UserId)`); the running session keeps one `UserController` per user
  participant, built from the store when the session loads and kept current through `UserUpdated` -
  the same way it keeps its `SessionController`, `ChannelController` and `AgentController`s. The
  `UserActor` gets that controller instead of a `User`. What the session shows about a user (the
  name) comes from the controller. Costs: the session and channel documents change shape; REST
  responses and the web UI that read `participants` must get names another way (resolved by the
  server, or a separate lookup); a user that no longer exists must be handled when a session loads.
- **Keep the copy, trimmed.** Store only `id` and `username`, refresh on `UserUpdated`. Smaller
  change, but still a copy that goes stale while the session is not running.
- **Agents.** `Participant::Agent(Agent)` has the same shape and the same staleness; whether agents
  follow in the same step or get their own issue is to be decided with the design.

## Done when

No session or channel document holds a copy of a user record; the running session's view of a user
is its `UserController`, current across renames and restarts.

## History

- **2026-09-29** — Opened while working on issue 94: once user records belong to the system and are
  readable by everybody, the copies in sessions are the remaining place where a user's `email` and
  `roles` travel with every session. The maintainer suggested that the session use a
  `UserController` to take changes in.
- **2026-09-29** — Agreed with the maintainer: (1) participants hold ids; (2) option A - the running
  session keeps one `UserController` and one `AgentController` per participant, handed to the
  actors; (3) agents in the same go; (4) names for REST read as the system; (5) a participant that
  no longer exists is left out. In two steps: ids first, with the actors loading their records
  themselves, then the controllers into the session.
- **2026-09-29** — Step 1: `Participant::User(UserId)` and `Participant::Agent(AgentId)`; the
  approver stays whole. Instantiating a template checks that the user exists and records the id.
  `ChannelLoop::create_actor` reads the user or agent from the store as the system; building a
  channel leaves out, with a `warn!`, a participant whose record is gone, while adding one that does
  not exist is an error. REST's `describe_participant` reads names from the store as the system, and
  shows `(unknown)` without a handle for a record that is gone. `ChannelRuntime::entity_store` is
  new.
- **2026-09-29** — Step 2, done: `ParticipantControllers` holds one `UserController` and one
  `AgentController` per participant of a session, loaded from the store as the system by
  `ControllerManager::participant_controllers` when the session starts; one whose record is gone is
  left out with a warning. The session hands them to its channels through `ChannelRuntime::new`, and
  `UserActor::new` / `AgentActor::new` take the controller instead of the record, so an agent in
  several channels of a session has one writer.
  `ActorRuntime::new_user_controller`/`new_agent_controller` and their "only your own" check are
  gone - the session hands every actor its own controller. The actors no longer subscribe to events
  about themselves: the `SessionLoop` subscribes to its topic plus its users' and agents' and keeps
  the controllers current, which leaves an actor only its channel to subscribe to (`OwnTopic` became
  `subscribe_channel`). Adding a participant whose record is gone is `ChannelError::ParticipantGone`
  (REST `404 participant_not_found`). `ChannelRuntime::entity_store` from step 1 is gone again.
  Still open, beyond this issue: an agent that takes part in two sessions has a controller in each,
  i.e. two writers - a process-wide controller per entity (option C) would end that.
- **2026-09-29** — The remaining two writers across sessions are issue
  [121](121-agent-controller-per-process.md).
- **2026-09-29** — `ParticipantControllers` from step 2 was replaced by one controller per agent and
  user in the instance, kept by the `ControllerManager` (issue
  [121](121-agent-controller-per-process.md)).
