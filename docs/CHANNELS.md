# Sessions, Channels and Actors

Caelumora uses an actor-based model: people and agents take part in
**channels** as **actors**, which exchange **envelopes**, and the channels of
one interaction form a **session**. This document describes how sessions and
channels are made, run and changed, how actors act in them, and how
moderators decide who responds. What an agent does in its turn is in
[AGENT-TURN.md](AGENT-TURN.md), how the channel's state is stored in
[STORAGE.md](STORAGE.md), and how clients follow a channel in
[REST-SERVER.md](REST-SERVER.md#streams).

## Sessions

A **session** is the container of one interaction. Its `SessionLoop` runs its
channels.

Every channel is a document of its own in the `EntityStore` (issue 70). The
session document lists its channels by id, in order (`Session.channels`).
Creating a session stores its channels with it (`SessionTemplate::build`
yields both), deleting a session deletes them, and loading a session loads
them. Loading happens in the runtime's name, since starting a session the
caller may read must not fail on a channel the caller is not in.

Every session has a privacy policy (`Session.privacy`, issue 134): whom it
lets read what was said in it from other sessions. It is taken over from its
template (`SessionTemplate.privacy`, private unless the template says
otherwise), changed only by the session's owner
(`SessionLoop::update_privacy`, stored first like a new name, announced as
`SessionUpdated`), and shown to everyone who takes part (`SessionView`).
What it decides is described in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#reading-another-channel).

### Creating and deleting channels

Channels can be created in and deleted from a running session:
`SessionLoop::create_channel` and `delete_channel`, over REST
`POST /sessions/{id}/channels` and `DELETE /sessions/{id}/channels/{cid}`.

*   **Creating** stores the channel (`CreateChannel`) and then adds its id to
    the session through `SessionController::add_channel`, which checks
    `UpdateSession`, so that only the owner may do it.
*   **Deleting** takes the channel out of the session (`remove_channel`) and
    deletes its document (`DeleteChannel`).

Only then is the running session changed and the change announced
(`ChannelCreated`, `ChannelDeleted`).

### One writer per document

Each document has one writer, which holds a lock while it changes and stores
it: the `SessionController` for the session, the `ChannelController` for its
channel. Renaming a channel (`ChannelLoop::update_name`, checked as
`UpdateChannel`) is stored first and only then taken over and announced
(`ChannelUpdated`). The controllers are described in
[STORAGE.md](STORAGE.md#controllers).

## Channels

A **channel** is one conversation within a session, such as a chat room. Its
`ChannelLoop`:

*   manages the lifecycle of the **actors** taking part in it;
*   routes **envelopes** between them;
*   applies a **moderator**, which decides who is asked to respond.

### Building a channel

Every channel is built by `Channel::build`, either from a template
(`ChannelTemplate::instantiate`) or on request. It adds an approver if the
channel has none - a template may name its own, with its strategy - and makes
every user and agent in the channel a contributor.

The participants of a requested channel come from the session's
participants, the creator always among them. Its moderator defaults to
`round_robin` and its classification to `internal` without compartments, as
in a template (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

A request that cannot be met is a `SessionError`, refused before anything is
stored: no name, a name the session already has, an unknown moderator,
someone who is not a participant of the session, or a classification the
policy refuses (`ActionType::Classify`: a compartment that is not configured).
The last is asked for channels from a template too.

### Running channels

`SessionLoop::run` starts every channel in its `JoinSet`: those the session
had and those created later, which it receives over a queue.

*   Stopping the session aborts `run`, and with it every channel.
*   Deleting a channel aborts that channel's task and deletes its state.
*   A channel deleted while it waits to be started is never started.

### Who may see a channel

A channel its viewer may not read (`GetChannel`: owner and participants) is
answered as absent and left out of lists and snapshots. The runtime decides
that, not the REST server: `SessionLoop::channel_for(ctx, id)` returns a
channel only to whoever may read it, and `SessionError::ChannelNotFound`
otherwise - the same as for one that does not exist. Every REST handler
reaches a channel through one `open_channel` (issue 102).

What a client lists and offers - which channels, which members, whether it may
change a channel - comes from views the runtime builds for one viewer (issue
80); see [SECURITY.md](SECURITY.md).

## Acting in a channel

Whoever writes or approves does so in their own name.
`ChannelLoop::user_actor(UserId)` returns *that user's* actor, and every
envelope is built by the actor itself, so sender, name and `SecurityContext`
follow from it. There is deliberately no way to reach "some" user actor.
Someone who is not a participant of a channel has no actor in it and is
refused with `403` (issue 30).

A frontend talks to the user's actor, never to the channel. The actor queues
what its user writes, approves or announces, and its task sends it on through
the transport `run` was given, like every actor's output; no actor takes a
way around it. The queue closes when the actor's task ends - also when it is
aborted with its channel - so that sending to a stopped channel fails instead
of waiting.

Waking the channel for a message is the actor's job, too: it sends a
`WakeupRequested` ahead of every message, in the message's span. Neither a
frontend nor the channel decides that by itself (issue 90).

User commands (`/help`, `/wakeup`) are run by the user's actor in the same
way; see [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

## Participants

Participants of the session can be added to and removed from a running
channel (issue 46). Changing the participants is an operation of the channel,
not of the session:

*   `ChannelLoop::add_participant(ctx, participant, history)`, over REST
    `POST /sessions/{id}/channels/{cid}/participants` with
    `{participant, history}`;
*   `ChannelLoop::remove_participant(ctx, participant)`, over REST
    `DELETE …/participants/{aid}`. It stops the participant's actor through
    `Actor::stop` (see [AGENT-TURN.md](AGENT-TURN.md)).

Like renaming, the change is made by the channel's one writer, the
`ChannelController` (`add_participant`, `remove_participant`). It recomputes
the channel's security attributes from its participants, stores the channel
(`UpdateChannel` - only the owner) and announces `AgentChannelJoined`/`Left`
or `UserChannelJoined`/`Left`. Events only announce; nothing changes a channel
because an event arrived.

Adding is additionally asked as `ActionType::JoinChannel` with the channel's
classification, where a policy can hold the channel's level against the
newcomer (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

### How much history a newcomer sees

Whoever adds a participant decides how much of the history so far they may
see: a `HistoryAccess` of all (the default), none, the last n messages or the
last n days. It is fixed at once as a `HistoryStart` (a message id) in
`Channel::history_limits`, so "the last three days" does not move day by day.

The model only holds these limits as data.
`ChannelLoop::visible_history(participant)` is the one place they are
applied. It answers with a
`VisibleHistory` - the messages, the digest of the history (up to where) and
the participant's start (from where) - which is used for the history over
REST, in the snapshot, for `read_history` (which records digest and start
together, see [MEMORY.md](MEMORY.md)) and for the transcript of a joining
agent.

*   A **joining agent** gets the visible part into its transcript.
*   A **rejoining agent** gets only what was said after the last message it
    has - what it missed while away. The transcript is only ever appended to,
    so older messages that a wider grant makes visible would land after the
    agent's own later answers. The agent reaches them through `read_history`,
    and its transcript stays in the order things were said.

Notes stay visible to everyone in the channel: they are written on paper, not
part of the conversation.

### What changes in the running channel

The `ChannelLoop` starts the new actor's task, tells the moderator and
rebuilds the `MentionDirectory`; a handle that becomes ambiguous is logged,
as at the start. The channel's run loop takes such changes over a queue and
ends once its last actor has ended.

A removed actor is stopped at once - an agent without finishing its turn - and
its state is kept, for the record and for a later return. The approver answers
a leaving agent's pending approvals as denied, so that nothing keeps waiting,
and decides the others again, since the number of users changed. The approver
itself cannot be removed.

### How others learn of it

A leave is announced on the event bus like every change of membership (issue
73). A removed user's live stream of the channel ends because it is theirs:
it is followed through their actor (`UserActor::watch`), which ends when it is
stopped, so no one has to listen for the announcement (issue 104).

Every channel response carries `can_change`, so that a client offers adding
and removing participants only where the user may (see
[WEB-UI.md](WEB-UI.md)).

## Acting on the channel as a whole

Two operations act on the channel as a whole. They ask the `SecurityManager`
first, one decision for the whole channel rather than one per actor.

### Cancelling

`ChannelLoop::cancel` (`ActionType::CancelChannel`) ends the current turn of
every actor - the web UI's stop button. It is an operation on the channel,
like renaming or deleting it, not an act of taking part: the policy decides
who may, and nobody has to be a participant for it (issue 102).

It is not a kill. A running tool finishes, the agent reflects, emits
`FullResponseGenerated` so that observers stop waiting, and returns to a
normal state with its history intact. A model call in flight, however, is
dropped rather than awaited: `Actor::cancel` is synchronous, flips the
agent's `ProcessingStateHolder` to `Cancelling` and cancels the turn's
`CancellationToken`. The streaming loop waits on that token next to the
model's stream, so the request is abandoned at once, wherever it stands
(issue 76). Waiting for an approval waits on it too: a turn waiting for a
decision ends at once, without one.

Lowering a channel's classification cancels its turns in the same way,
as a consequence of the reclassification; each agent then gives its turn
up instead of taking it over (issue 141, see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

Afterwards the channel is *quiet* until it is woken. What is said is still
delivered, so that every agent's transcript keeps up with the history, but
nobody is asked to respond to it.

### Waking

A quiet channel is woken in one of two ways:

*   **Deliberately**: `ChannelLoop::wakeup` (`ActionType::WakeChannel`), as
    the command `/wakeup` does; `wakeup_actor` wakes a single actor under the
    same permission.
*   **Because something is on its way**: a `WakeupRequested` that a user's
    actor sends ahead of its next message, or a user's mention. This happens
    as the channel routes it (`wake_for_delivery`) and asks nothing. It
    follows from an action that was already permitted, and a user allowed to
    write but not to wake would otherwise be unable to write.

### Refusals

A refusal is a `ChannelError::Forbidden`, which
`caelumora_runtime::error::is_forbidden` recognises, so that REST answers
`403` rather than `500`. Like a refused tool call, it says what was refused,
not which rule refused it.

## Actors and envelopes

An **actor** is anything that sends and receives events in a channel.

*   **`Actor`** (`caelumora_runtime::actor`) is the trait of all actors. Its
    task is `async fn run`, which receives envelopes and sends its output
    through the transport it is given. `wakeup`, `cancel` and `stop` are
    synchronous, because they come from other tasks and must take effect at
    once; `message_context` answers what an agent's model saw (see
    [AGENT-TURN.md](AGENT-TURN.md)). The implementations are `AgentActor`,
    `UserActor` and `ApproverActor` (`caelumora_runtime::actor_impl`).
*   **`ActorId`** identifies an actor: a `User`, an `Agent` or an `Approver`,
    or `System` - the runtime itself, set only deliberately through
    `SecurityContext::system()`.
*   **`Envelope`** is the unit of communication: a timestamp, a
    `SecurityContext`, an `EventContext` and the `ActorEvent` itself.

## The channel and actor runtime

A `ChannelLoop` is built with a `ChannelRuntime`, not with the
`RuntimeManager` itself. It carries what everything in the channel shares:

*   the process-wide services of the `RuntimeManager`;
*   the user commands of the session: a `CommandExecutor`, built once per
    session by `RuntimeManager::new_command_executor`;
*   a handle to the channel. It is a `Weak`, set once by `ChannelLoop::new`
    when the channel exists: the channel owns its actors, and a strong
    reference back would keep a stopped channel alive.

The `ChannelRuntime` is the only source of `ActorRuntime`s, so every actor
belongs to exactly one channel.

An `ActorRuntime` is made for one actor
(`ChannelRuntime::new_actor_runtime(actor, session, channel)`) and hands out
only what concerns that actor (issue 69):

*   the services every actor needs: `security_manager`, `secret_manager`,
    `component_registry`, `command_executor` - the model is not among them,
    an agent has it as a component (issue 130);
*   its personality (`personality`);
*   its own agent state (`agent_state`) and its state in the channel
    (`load_channel_state` / `store_channel_state`);
*   its own event subscription (`subscribe_channel` - its channel, nothing
    else) and announcement (`announce_channel_state_changed`);
*   the loop of its channel (`channel_loop`, `pub(crate)`), and a stored
    session as a given context may read it (`load_session`);
*   a `SessionReader` that enforces the confidentiality rules (see
    [MEMORY.md](MEMORY.md)).

The full `ChannelLoop` and `SessionLoop` are `pub(crate)`, so components from
other crates cannot reach them; within the runtime, the user actor needs the
channel for commands, and the session reader needs both.

Components are not given their actor's `ActorRuntime`, only what their
`ComponentContext` holds
([128](../issues/128-actor-runtime-in-components.md)); see
[COMPONENTS.md](COMPONENTS.md).

## Moderators

A moderator decides how messages are routed and who is asked to respond. A
channel's moderator is named in its template (`ChannelTemplate.moderator`,
default `round_robin`) and built by `RuntimeManager::new_moderator`.

All moderators route the same way:

*   every message goes to every other participant;
*   approvals, failed turns and announced commands go to whom they concern;
*   every agent a user `@`-mentioned is asked to respond.

They differ only in whom they ask when a message mentions no agent. That
difference is a `ResponderPolicy`, and `PolicyModerator` is everything else
(`caelumora-runtime/src/moderator_impl/`), so a new moderator is a new
policy:

| Moderator | Who responds to a message without a mention |
|---|---|
| `broadcast` | every agent |
| `round_robin` | the agents take turns, in the order of their ids; a mention does not take a turn |
| `random` | one agent, chosen at random |
| `mentions_only` | nobody: agents respond only when a user mentions them |

The REST API returns each channel's `moderator`, which the web UI explains
to the user (see [WEB-UI.md](WEB-UI.md)); the TUI prints it when the chat
starts.

### `@`-Mentions

A user can address participants directly with `@handle` (e.g. `@sassy` for
"Sassy"). A handle is the participant's name lowercased, with every run of
other characters collapsed into `-` (`caelumora_model::mention`). Unknown
handles stay plain text, and text in backticks is ignored.

The `ChannelLoop` resolves mentions once, as a message enters the channel,
and stores the resolved `ActorId`s in `Message::mentions`. Nothing downstream
parses the text again, so a later rename cannot change whom a past message
addressed.

A mention changes who is asked to respond, not who receives the message:
every participant still gets it, but every moderator (through the shared
`ModeratorActors::mentioned_responders`) sends `ResponseRequested` to exactly
the mentioned agents instead of applying its own policy. Mentioned agents
that are asleep are woken first.

Only mentions written by a user are routed. Mentions in agent responses are
resolved and rendered but do not steer the moderator, so agents cannot
address each other in an unbounded loop.

The REST API lists each channel's `members` in no particular order, each with
its handle (`null` for participants that cannot be mentioned, such as
approvers), which the web UI uses for `@` completion and highlighting.
