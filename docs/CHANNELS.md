# Sessions, Channels and Actors

Caelumora uses an actor-based model: people and agents take part in **channels** as **actors**,
which exchange **envelopes**, and the channels of one interaction form a **session**. This document
describes how sessions and channels are made, run and changed, how actors act in them, and how
moderators decide who responds. What an agent does in its turn is in [AGENT-TURN.md](AGENT-TURN.md),
how the channel's state is stored in [STORAGE.md](STORAGE.md), and how clients follow a channel in
[REST-SERVER.md](REST-SERVER.md#streams).

## Sessions

A **session** is the container of one interaction. Its `SessionLoop` runs its channels.

Every channel is a document of its own in the `EntityStore` (issue 70). The session document lists
its channels by id, in order (`Session.channels`). Creating a session stores its channels with it
(`SessionTemplate::build` yields both), deleting a session keeps them - hidden with it, since a
channel is reached only through its session (issue 164) -, purging it removes them, and loading a
session loads them. Loading happens in the runtime's name, since starting a session the caller may
read must not fail on a channel the caller is not in.

What lies in a session's scope - the personalities, session templates and agents of
`Scope::SessionScope`, and those of its channels' scopes - goes with it (issue
[178](../issues/178-scoped-lookup-by-name.md), `caelumora-runtime/src/scoped.rs`): deleting the
session deletes them, restoring it restores those deleted with it - not one deleted by hand before,
nor one whose name was taken meanwhile (issue [250](../issues/250-unique-names-per-name-space.md))
-, purging it purges them, an agent with its state. That is done in the runtime's name, as a
consequence of what was permitted; a deleted or restored agent is announced (`AgentDeleted`,
`AgentRestored`). Its agents take part in no other session, so they leave no channel.

That holds because what lies in a session's or a channel's scope is used only within it
(`Scope::admits`): an agent of it takes part only in that session's channels, or in that channel -
creating, restoring or adding to a channel refuses one of another (`ChannelError::OutOfScope`,
`Member::require_in_scope`) -, and a personality of it makes agents only within it
(`TemplateError::PersonalityOutOfScope`). What lies in a user's scope or everywhere is used wherever
the permissions allow: that scope only says where its name is found. A session template lies only
in a user's scope or everywhere; one of a session's or a channel's would make sessions it can never
lie in, and is not stored (`Stored::validate`, `StoreError::Validation`).

Every session has a privacy policy (`Session.privacy`, issue 134): whom it lets read what was said
in it from other sessions. It is taken over from its template (`SessionTemplate.privacy`, private
unless the template says otherwise), changed only by the session's owner
(`SessionLoop::update_privacy`, stored first like a new name, announced as `SessionUpdated`), and
shown to everyone who takes part (`SessionView::privacy`). What it decides is described in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#reading-another-channel).

### Creating and deleting channels

Channels can be created in and deleted from a running session: `SessionLoop::create_channel` and
`delete_channel`, over REST `POST /sessions/{id}/channels` and `DELETE
/sessions/{id}/channels/{cid}`.

*   **Creating** stores the channel (`CreateChannel`) and then adds its id to the session through
    `SessionController::add_channel`, which checks `UpdateSession`, so that only the owner may do
    it. A channel's name is unique in its session: the store refuses one the session has
    (`NameTaken`, issue [250](../issues/250-unique-names-per-name-space.md)), when the channel is
    created, renamed or restored.
*   **Deleting** takes the channel out of the session (`remove_channel`) and marks its document
    deleted (`DeleteChannel`, issue 164). The channel and its state are kept. What lies in its scope
    (`Scope::ChannelScope`) is deleted with it, as with a session; an agent of it takes part in no
    other channel, so it leaves none.
*   **Restoring** a deleted channel (`SessionLoop::restore_channel`) asks `UpdateSession` and its
    participants' clearance, as creating one does, takes the mark away (`Restore`), lists it in
    the session again and starts it from its stored state. Its name must still be free. What
    lies in its scope and was deleted with it comes back - but for one whose name was taken
    meanwhile, which stays deleted.
*   **Purging** is not a channel's: a deleted channel is removed for good - document and state -
    with its session (`SessionManager::purge_session`), which needs no running loop.

Only then is the running session changed and the change announced (`ChannelCreated`,
`ChannelDeleted`, `ChannelRestored`; a purge announces nothing).

A deleted channel stops its actors before its task is aborted (issue 187).
`ChannelLoop::stop_actors` tells every actor to `stop` and waits until their tasks have ended, at
most `STOP_GRACE`. In that time an external agent cancels its open remote tasks and every actor
stores its state. Only then is the channel's task aborted, and with it whatever still runs; the
state they stored is kept with the deleted channel.

### Stopping an actor

`Actor::stop` is synchronous and goes past the events, like `cancel`: it takes effect at once, not
after what is queued. Every actor ends its own task when stopped, which is why `stop` has no
default. Waiting for that does not go through `run` either: `run`'s `JoinSet` alone owns the
actors' tasks, so that they end with the channel's own task. Each actor's task is additionally
tracked by a `TaskTracker` in `ChannelLoop::actors`, which whoever stopped the actor waits on.
Removing a participant and deleting the channel use the same grace: an actor that has not ended
after `STOP_GRACE` is aborted. A tool that hangs no longer keeps an agent: a stop interrupts it, and
the agent gives its turn up within the grace, the call recorded as having run (issue 161).

### One writer per document

Each document has one writer, which holds a lock while it changes and stores it: the
`SessionController` for the session, the `ChannelController` for its channel. Renaming a channel
(`ChannelLoop::update_name`, checked as `UpdateChannel`) is stored first - where a name the session
has is refused - and only then taken over and announced (`ChannelUpdated`). The controllers are
described in [STORAGE.md](STORAGE.md#controllers).

## Channels

A **channel** is one conversation within a session, such as a chat room. Its `ChannelLoop`:

*   manages the lifecycle of the **actors** taking part in it;
*   routes **envelopes** between them;
*   applies a **moderator**, which decides who is asked to respond.

### Building a channel

Every channel is built by `Channel::build`, either from a template (`ChannelTemplate::instantiate`)
or on request. It adds an approver if the channel has none - a template may name its own, with its
strategy - and makes every user and agent in the channel a contributor.

The participants of a requested channel come from the session's participants, the creator always
among them. Its moderator defaults to the `policy_moderator` with the `round_robin_policy` and its
classification to `internal` without compartments, as in a template (see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md) and [Moderators](#moderators)).

A request that cannot be met is a `SessionError`, refused before anything is stored: no name, a name
the session already has, someone who is not a participant of the session, or a classification the
policy refuses (`ActionType::Classify`: a compartment that is not configured). The last is asked for
channels from a template too. Components that make no moderator (`SessionError::InvalidModerator`)
are found when the channel is built, after it was stored; it is then taken back out of the session
and deleted.

### Its language

A channel may be held in a language (`Channel::language`, issue 56): a BCP 47 tag (`Language`, such
as `de` or `en-GB`), checked by its form, kept as written and compared in its usual case (`de-ch` is
`de-CH`). A channel created on request or from a template is held in the language its creator
prefers (`UserProfile::preferred_language`) unless the request names another
(`ChannelSpec::language`) or its template does (`ChannelTemplate::language`, for a channel whose
purpose comes with its language, such as a German quiz); one whose creator prefers none is held in
none in particular. Whoever may change the channel changes it or takes it back
(`ChannelLoop::update_language`), stored like a new name - where `UpdateChannel` is asked - and
announced as `ChannelUpdateEvent::LanguageChanged`. Its agents are told by a prompt part (see
[AGENT-TURN.md](AGENT-TURN.md#the-channels-language)); nothing said before is translated.

### What it is for

A session and a channel may say what they are for (issue 245): a `summary`, one line seen at a
glance, and a `briefing`, what an agent should know before it works there - conventions, context,
what to do and what not, like an `AGENTS.md`. Both are optional fields of `Session` and `Channel`,
set in the template (`SessionTemplate` and `ChannelTemplate` have them, copied into what they make;
`SessionTemplate::description` stays the catalogue line) and changed by whoever may change the place
(`SessionLoop::update_description`, `ChannelLoop::update_description`; `UpdateSession`,
`UpdateChannel`), announced as `DescriptionChanged` of `SessionUpdateEvent` and
`ChannelUpdateEvent`. Their types say what they may be (`Summary`, one line of at most 300
characters; `Briefing`, at most 16 000 - a briefing is in the context of every agent there on every
turn): not empty, not of whitespace alone, no control characters, and kept exactly as written -
trimming is the input's, as the web UI does (`caelumora_model::value::text`, issue
[248](../issues/248-validated-text-fields.md)). They are passed on as strings - from a request to
the loop, the controller and in `DescriptionChanged` - and made where they are assigned to the
model (`Session::describe`, `Channel::describe`, an empty one being none), which the controller
does as it applies the event; and they are checked when a document is read, so a template written
by hand is checked when the seed is loaded.

A session's descriptions are public, as its name is: everybody in the session reads them, and every
agent in each of its channels, external ones too. A channel's are part of the channel and classified
as it is now. The snapshots of a session and a channel carry the summary always and the briefing
with `Detail::BRIEFING`, which lists leave out. Agents are told both by a prompt part (see
[AGENT-TURN.md](AGENT-TURN.md#what-the-session-and-channel-are-for)).

### Its workspace

A session may have one workspace attached (`Session::workspace`, issue
[253](../issues/253-workspace.md)): a tree of files of a user's, shared by the session's channels
(issue [251](../issues/251-open-terminal.md), decisions 10 to 13). A template may name it
(`SessionTemplate::workspace`, a `WorkspaceTemplate`): found by its name in the scope the template
asks for, if it is in the storage the template names - another is an error - or made there by the
session's owner, as an agent of a template is. Its scope decides its life: one in the session's
scope is made for every session anew, used within it alone (`Scope::admits`) and deleted,
restored and purged with it, its files too; one in the owner's scope or the global one is found by
every session of theirs that names it.

Otherwise only the workspace's owner attaches it, to a session they may change
(`SessionLoop::set_workspace`: `UpdateSession`, then `GetWorkspace` and `UpdateWorkspace`) - not one
of another session's scope (`SessionError::WorkspaceOutOfScope`); whoever may change the session
takes it away, which shares nothing. Announced as `WorkspaceChanged` of `SessionUpdateEvent`.

In a session the workspace is used through a channel, by its current members - a person, or an
agent acting there for whoever asked it - and nobody else, also not its owner from outside the
channel. A channel uses it where

*   its switch says so (`Channel::uses_workspace`, on unless its template or whoever may change the
    channel says otherwise: `ChannelLoop::update_uses_workspace`, `UpdateChannel`,
    `UsesWorkspaceChanged`) - what the channel is for; and
*   the workspace and the channel flow both ways (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#workspaces)) - the boundary, which the switch cannot
    widen.

So a session for a request can have a channel with the person who asked, classified below the
workspace or with the switch off, and channels with helpers that work on its files. A view of a
channel offers the workspace only where its viewer may use it there (`ChannelView::workspace`),
and says nothing of it otherwise; using it asks again, of the session and the channel as they are
then (`WorkspaceView::terminal`). Its owner works on it outside a session too, unless it lies in a
session's scope.

### Running channels

`SessionLoop::run` starts every channel in its `JoinSet`: those the session had and those created
later, which it receives over a queue.

*   Stopping the session aborts `run`, and with it every channel.
*   Deleting a channel aborts that channel's task; its state is kept until it is purged.
*   A channel deleted while it waits to be started is never started.
*   A participant that cannot be run is left out when the channel starts, with a warning, and the
    channel runs with everyone else: a user or agent deleted since it joined (issue 120), or an
    agent whose personality was withdrawn from its owner or deleted (issue 198) - a third party, the
    personality's owner, must not keep someone's channel from starting. Such an agent is still a
    participant; adding it to another channel is refused.

### Who may see a channel

A channel its viewer may not read (`GetChannel`: owner and participants) is answered as absent and
left out of lists and snapshots. The runtime decides that, not the REST server:
`SessionLoop::channel_for(ctx, id)` returns a channel only to whoever may read it, and
`SessionError::ChannelNotFound` otherwise - the same as for one that does not exist. Every REST
handler that acts on a channel reaches it through one `open_channel` (issue 102); one that reads it
goes through the views (`SessionView::channel`, issue 163), which answer the same way.

What a client lists and offers - which channels, which members, whether it may change a channel -
comes from views the runtime builds for one viewer (issue 80); see [SECURITY.md](SECURITY.md).

## Acting in a channel

Whoever writes or approves does so in their own name. `ChannelLoop::user_actor(UserId)` returns
*that user's* actor, and every envelope is built by the actor itself, so sender, name and
`SecurityContext` follow from it. There is deliberately no way to reach "some" user actor. Someone
who is not a participant of a channel has no actor in it and is refused with `403` (issue 30).

A frontend talks to the user's actor, never to the channel. The actor queues what its user writes,
approves or announces, and its task sends it on through the transport `run` was given, like every
actor's output; no actor takes a way around it. The queue closes when the actor's task ends - also
when it is aborted with its channel - so that sending to a stopped channel fails instead of waiting.

The channel does not take an actor's word for whose name it speaks in (issue 228). Each actor is
given an `OriginEventSender`, which passes on only an envelope that speaks as the actor
(`OriginEventSender::speaks_as`): one that names it as its sender (`EventContext::sender_id`), as
the caller of its `SecurityContext`, and as the sender of every message it carries. Anything else is
refused with `TransportError::Refused` and logged as an error - it is a bug of the actor, which
learns of it where it happened. Who the actor acts for - the context's initiator - may be someone
else: an agent answers on behalf of whoever asked it. What the channel says itself - a moderator's
notice, `ParticipantRemoved` - does not come through an actor's sender.

**Nothing said is unsaid** (issue 258). The channel keeps every message it admits in its history
as it arrives - `CompleteMessageGenerated`, and the steps of a plan as
`IntermediateMessageGenerated` - and records how each turn ended after its messages, from the
event of that name: `TurnFinished`, or `TurnGivenUp` with its reason. A turn given up
keeps what it said; only a message still streaming when it ended is lost, never having been
complete. A message is admitted as it arrives (`admits`), so one said after the channel was lowered
below its turn is refused, and what was said before is fitted with the rest. An agent removed while
it answers can no longer tell the channel: `remove_participant` asks for its turn under way before
it stops it, and the channel records that turn as given up, `Stopped`, and announces it in the
participant's name after `ParticipantRemoved`.

Waking the channel for a message is the actor's job, too: it sends a `WakeupRequested` ahead of
every message, in the message's span. Neither a frontend nor the channel decides that by itself
(issue 90).

User commands (`/help`, `/wakeup`) are run by the user's actor in the same way; see
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

## Changing what was said

What was said can be changed by its author, but never rewritten
([234](../issues/234-modify-revoke-and-retry-messages.md)): a message is *modified* - it reads
differently from now on, at the place it has - or *taken back*. Nothing forks: the others saw the
message, and what they said in reply stays.

*   **A user** changes their message through their actor, as they write:
    `UserActor::modify_message(message, content, classification)` and `revoke_message(message)`. The
    actor looks the message up in the channel's history and asks before it sends - whether the user
    may change it (`ModifyMessage`, `RevokeMessage`; a message is its author's), then for a
    modification whether it may enter the channel so classified (`Publish`) - so that the user
    learns of a refusal: `ChannelError::MessageNotFound`, `Forbidden`, `MessageRevoked` for one
    taken back already, `ClassificationRefused`. It then sends `ActorEvent::MessageModified`, the
    message as it reads from now on, or `MessageRevoked { message, at }` through its transport, as
    every actor's output.
*   **The channel admits it** as it admits a message (`ChannelLoop::admits_change`), in the
    sender's name: the message is in its history, its author may change it, and it can be changed
    so as it stands - not taken back, and modified by its own sender in its own role. Anything else
    is dropped and logged. It records the change in its history (`MessageHistory::modify`,
    `revoke`), which then shows the message as it stands: modified, with when (`Message::modified`),
    or without what it said (`Message::revoked`).
*   **Everyone who heard it follows.** The moderator routes the change to every user and agent but
    its sender (`route_to_participants`) and asks nobody to respond. An agent of either kind takes
    it in between turns, in order with what it hears (`actor::intake::take_in`): into its history if
    that holds the message, into its transcript if the message is on its active path - a
    modification as it hears it, through its perceive processors; where nothing of it passes, the
    message is no longer given. A turn under way ends as it began; the next is given what stands.
    An agent that never held the message records nothing. An external agent's transcript holds only
    what the remote agent has not been given yet, and A2A has no way to change what was sent: a
    change of a message the remote agent was given is told to it with its next call instead, by a
    notice at its place among what was said since, in the name of its author - modified to what, or
    taken back ([236](../issues/236-external-agent-not-told-of-changes.md), see
    [AGENT-TURN.md](AGENT-TURN.md#changes-to-a-message)). That only asks: the remote agent decides
    whether the old version stops acting.
*   **Reclassifying** a message goes the same way
    ([239](../issues/239-reclassified-message-not-in-agents-state.md)):
    `UserActor::reclassify_message(message, classification)` asks `Reclassify` - its author, or the
    channel's owner for someone else's, never above the channel - and sends
    `ActorEvent::MessageReclassified`; the channel admits it as the policy says, records it, and it
    reaches everyone who heard the message, whose next turns are labelled by it as it stands (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reclassification)).
*   **Taking back is not erasing.** The message stays in every participant's record, in the
    contexts of what was answered to it, and with a remote agent that was given it (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#messages)).

How the chains record a change is described in
[AGENT-TURN.md](AGENT-TURN.md#changes-to-a-message).

### Answering again

Whoever takes part may ask an agent to answer again instead of one of its messages - its last
answer, and nothing else, since a new answer cannot be placed above what answered the old one
without forking - or instead of its answer still under way (issue 235). It is a request to the
agent, not an operation of the channel: `ActorEvent::RetryRequested { target, author }`, whose
`RetryTarget` is a message (`Message(id)`) or a turn (`Turn(id)`, the id `ResponseStarted`
announced and every message of the turn carries).

*   **A user** asks through their actor: `UserActor::request_retry(message)` or
    `request_retry_of_turn(author, turn)` asks `RetryMessage` of the channel first - as cancelling
    and waking, whoever may read the channel - and sends a `WakeupRequested`, then the request.
    `ChannelError::MessageNotFound`, `Forbidden`, `MessageRevoked` and `NotAnAgentsMessage` tell
    the user why not for a message; for a turn, `NotAParticipant` if `author` is no agent of the
    channel and `NotUnderWay` if the turn is not its turn under way (`ChannelLoop::turn_under_way`):
    it ended, or never was. So a request about an answer that is over is refused at once, without
    an event.
*   **The channel** admits it as it admits a change (`admits_change`): its sender may ask it, and
    `author` is the agent that said the message, which was not taken back - or, for a turn, an
    agent of the channel. The moderator routes it to `author` alone, whom the channel wakes first,
    as it wakes whom it asks to respond. A turn is ended as the request is delivered:
    `Actor::retry_turn(turn)` gives it up if it is still the agent's turn under way, decided in one
    step under the turn's lock in its `RunControl`; if it is not - it ended between the user's
    actor asking and the delivery - the request is dropped and logged, and its users see the answer
    end. Whether a retry is possible is decided before anything is interrupted: a request about an
    older message never ends the answer under way, it waits for it like everything the agent hears.
*   **The agent**, its turn under way asked for again, ends the step under way at once - the model
    call is dropped, a running tool interrupted, a wait for an approval ended, as on a cancel - and
    gives the turn up (`GivenUpReason::Retried`, issue 145): its messages stay in its transcript
    where they were said, followed by why it ended (issue 258), so its model sees the calls that
    ran; its approval requests are withdrawn. What it said stays said, in the channel's history
    too, followed by its end, so nothing has to be taken back (issue 258). A cancel asked before
    wins: the turn is taken over instead.
*   **The agent** takes requests in between turns, with what it hears (`actor::intake::take_in`):
    *   the request of the turn it gave up for it is accepted, whatever came with it - its answer is
        gone already - and answered with `RetryAccepted`;
    *   a request of a turn it did not give up for it - a cancel came first, or the turn failed -
        is rejected as `Ended`;
    *   of the requests of a message, the last is judged against the conversation as it stands -
        unless the request of a turn was accepted, which answers anew anyway - and the others are
        rejected as asked again. It answers again only if the message is its last answer, as its
        own transcript says (`AgentChannelState::retried`): an answer of its own on the
        transcript's active path, after which only the rest of its turn (`Message::turn_id`) and
        changes to what was said follow. It then says so with `RetryAccepted` and takes back the
        message and the rest of its turn - in its own state, and with `MessageRevoked` for what the
        channel holds. Otherwise
        it says why with `RetryRejected`, whose `reason` is a `RetryRefused` - not its answer, more
        was said or done since, calls of tools that ran would be taken back, asked again since, its
        answer had ended, or an external agent - one of a known few, which a user interface words
        itself.

    A retry accepted and a request to respond in the same batch are one turn, in the request's
    contexts - what was said during the turn given up is answered with it; a retry alone begins
    one in its own. Its model is given the conversation as it stands, without what was replaced.
    What it then says is routed as any answer.
*   **An external agent** rejects every request to answer again: its remote agent keeps what it was
    given, and nothing here takes the answer back from it. It has no turn under way here
    (`Actor::turn_under_way` is `None`), so a request of its answer under way is refused by the
    user's actor.

`RetryAccepted` and `RetryRejected` go to the users and are never written to the history. Whether a
viewer may ask is part of each member's snapshot (`MemberSnapshot::can_retry`,
`MemberView::may_retry`, issue 242): the viewer's `RetryMessage` in the channel, of an agent taking
part whose kind answers again (`PersonalityKind::can_retry`) - not of an external agent, which
would only refuse. So a client offers it only where it is permitted and can succeed. The TUI asks
it, with `/retry`, of the answer under way it was told of last, else of the last answer of an agent
it showed.

## Participants

Participants of the session can be added to and removed from a running channel (issue 46). Changing
the participants is an operation of the channel, not of the session:

*   `ChannelLoop::add_participant(ctx, participant, history)`, over REST `POST
    /sessions/{id}/channels/{cid}/participants` with `{participant, history}`;
*   `ChannelLoop::remove_participant(ctx, participant)`, over REST `DELETE …/participants/{aid}`. It
    stops the participant's actor through `Actor::stop` (see [AGENT-TURN.md](AGENT-TURN.md)).

Like renaming, the change is made by the channel's one writer, the `ChannelController`
(`add_participant`, `remove_participant`). It recomputes the channel's security attributes from its
participants, stores the channel (`UpdateChannel` - only the owner) and announces
`AgentChannelJoined`/`Left` or `UserChannelJoined`/`Left`. Events only announce; nothing changes a
channel because an event arrived.

A removed participant is kept as a former one (`Channel::former`, issue 164): `FormerParticipant`
with who removed it and when (`Removal`), not among `participants`, which is who takes part now -
so nothing that reads them changes. Added again, it is taken out of `former`; its history limit is
not kept, it is given a new one. The views reach former members from the document
(`ChannelView::former_members`, `ChannelView::member`, `MemberView::left`): what an agent said and
saw stays readable for whoever may read its context. An agent that left before the channel kept who
left is still found by the messages it left in the history.

Deleting an agent (`SessionManager::delete_agent`, `DeleteAgent`, issue 164) makes it leave every
channel it takes part in, running or not, as a former participant - removed in the runtime's name,
since deleting the agent was permitted; who deleted it is the agent's own mark. Only the channels of
sessions it takes part in are looked at; those of deleted sessions keep it, and leave it out if
they are restored, as any participant that no longer exists. A deleted agent keeps its name in the
views, so that what it said keeps its sender. Restoring it (`restore_agent`) brings it back into no
channel - it is added again where it is wanted - and is refused if another agent took its name in
its scope meanwhile. Purging it (`purge_agent`) removes its document and its state, its own and in
every channel. An agent of a session's or a channel's scope is also deleted, restored and purged
with that session or channel (see [Sessions](#sessions)).

Adding is asked first as `UpdateChannel`, before anything of the newcomer is looked at (issue 198),
and additionally as `ActionType::JoinChannel` with the channel's classification, where a policy can
hold the channel's level against the newcomer (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

### How much history a newcomer sees

Whoever adds a participant decides how much of the history so far they may see: a `HistoryAccess` of
all (the default), none, the last n messages or the last n days. It is fixed at once as a
`HistoryStart` (a message id) in `Channel::history_limits`, so "the last three days" does not move
day by day.

The model only holds these limits as data. `ChannelLoop::visible_history(participant)` applies them
for a running channel, `ChannelView::history()` for a stored one too (issue 163). Both answer with a
`VisibleHistory` - the messages, the digest of the history (up to where) and the participant's start
(from where) - which is used for the history over REST, in the snapshot, for `read_history` (which
records digest and start together, see [MEMORY.md](MEMORY.md)) and for what a joining actor is
given.

*   A **joining actor** is given the visible part (`Actor::catch_up`, issue 190) and takes it in as
    what it hears: an agent of either kind keeps all of it in its history and what passes its
    perceive processors in its transcript (issues 196, 206) - an external agent gives that to the
    remote agent when it is next asked. Users and approvers keep nothing.
*   A **rejoining actor** keeps only what was said after the last message it has - what it missed
    while away (`MessageHistory::missing_from`). The history is only ever appended to, so older
    messages that a wider grant makes visible would land after the agent's own later answers. The
    agent reaches them through `read_history`, and what it keeps stays in the order things were
    said.

Notes stay visible to everyone in the channel: they are written on paper, not part of the
conversation.

### What changes in the running channel

The `ChannelLoop` starts the new actor's task, tells the moderator and rebuilds the
`MentionDirectory`; a handle that becomes ambiguous is logged, as at the start. The channel's run
loop takes such changes over a queue and ends once its last actor has ended.

A removed actor is stopped at once - an agent without finishing its turn - and its state is kept,
for the record and for a later return. The approver answers a leaving agent's pending approvals as
denied, so that nothing keeps waiting, and decides the others again, since who may approve them
changed (issue 14). The approver itself cannot be removed.

A participant added again while its old actor still ends - storing its state, at most `STOP_GRACE`
before it is aborted - waits for it (issue 171): `ChannelLoop` keeps the task of every removed actor
(`leaving`), and `add_participant` waits on it before it changes or loads anything, at most a
little past the grace (`LEAVE_GRACE`); an actor that has not ended even then is answered with
`ChannelError::StillLeaving` (REST `409`, `still_leaving`). The new actor thus loads the state as
the old one left it, and never runs next to it. The run loop takes the events its actors sent
before a change of its actors (`biased`), so that what the old actor sent before it ended is
routed - or dropped, as a removed actor's - before the new one starts under the same id. An actor
removed before the run loop started it is never started.

### How others learn of it

A leave is announced on the event bus like every change of membership (issue 73). In the channel
itself, the removal is announced with `ActorEvent::ParticipantRemoved`, in the name of whoever
removed the participant - `System` if the runtime did. The run loop sends it as it stops the actor,
between the events it routes, and from then on drops whatever the actor still sends while it
finishes its step: what the participant sent before is in the channel, nothing of it follows the
announcement. That is how a frontend knows that a turn it shows has ended - a stopped agent
announces nothing itself, it has left (issue 162). Like `TurnGivenUp`, every moderator forwards it
to the users only, and it is never stored.

A removed user's live stream of the channel ends because it is theirs: it is followed through their
actor (`UserActor::watch`), which ends when it is stopped, so no one has to listen for the
announcement (issue 104).

Every channel response carries `can_change`, so that a client offers adding and removing
participants only where the user may (see [WEB-UI.md](WEB-UI.md)).

## Acting on the channel as a whole

Two operations act on the channel as a whole. They ask the `SecurityManager` first, one decision for
the whole channel rather than one per actor.

### Cancelling

`ChannelLoop::cancel` (`ActionType::CancelChannel`) ends the current turn of every actor - the web
UI's stop button. It is an operation on the channel, like renaming or deleting it, not an act of
taking part: the policy decides who may, and nobody has to be a participant for it (issue 102).

It is not a kill. The agent reflects, emits `TurnFinished` so that
observers stop waiting - the users get it even when the turn said nothing (issue 225) - and returns
to a normal state with its history intact. A model call in flight, however, is dropped rather than
awaited: `Actor::cancel` is synchronous, leaves the agent's run loop an interrupt and cancels the
turn's `CancellationToken`. The streaming loop waits on that token next to the model's stream, so
the request is abandoned at once, wherever it stands (issue 76). Waiting for an approval waits on it
too: a turn waiting for a decision ends at once, without one, and withdraws its requests (see
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md)). The run loop takes the interrupt after the step it
cut short and goes on in `Cancelling`, whatever that step returned; `stop` works the same way with
an interrupt of its own, which wins (issue 152). A running tool is not waited for either: the call
is answered as interrupted, and no further call of the step is started (issue 161, see
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#running-tool-calls)).

Lowering a channel's classification cancels its turns in the same way, as a consequence of the
reclassification; each agent then gives its turn up instead of taking it over (issue 141, see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

Afterwards the channel is *quiet* until it is woken. What is said is still delivered, so that every
agent's transcript keeps up with the history, but nobody is asked to respond to it.

`ChannelLoop::cancel_actor` ends the turn of one actor alone (issue 235) - the stop icon on an
answer under way, or on a working agent in the members panel, in the web UI; who may is part of each
member's snapshot (`MemberSnapshot::can_cancel`, `MemberView::may_cancel`, issue 243). It is
cancelling, under the same permission (`CancelChannel`), asked before the actor is looked up, so
that who may not learns nothing of who is in the channel; an actor that is not there is
`ChannelError::NotAParticipant`, as for `wakeup_actor`. The turn ends as on a cancel of the channel,
but the channel does not become quiet: being quiet is the channel's state, not one actor's. The
others go on answering and are asked to respond as before, and the answer the cancelled turn is
taken over with reaches everyone as any answer does - another agent may respond to it.

### Waking

A quiet channel is woken in one of two ways:

*   **Deliberately**: `ChannelLoop::wakeup` (`ActionType::WakeChannel`), as the command `/wakeup`
    does; `wakeup_actor` wakes a single actor under the same permission. The channel moves a
    sleeping agent to `Idle` itself; the agent announces it (`StateChanged`, see
    [AGENT-TURN.md](AGENT-TURN.md#the-cognitive-cycle), issue 243).
*   **Because something is on its way**: a `WakeupRequested` that a user's actor sends ahead of its
    next message. This happens as the channel routes it (`wake_for_delivery`) and asks nothing. It
    follows from an action that was already permitted, and a user allowed to write but not to wake
    would otherwise be unable to write.

A single agent is also woken when the moderator asks it to respond: a sleeping agent ignores a
`ResponseRequested`, so `route_event_impl` wakes everyone a decision asks before it delivers
anything of that decision. The policies never choose a sleeping agent (`actor_candidates`), so this
wakes only agents a message mentions; an agent that is not addressed keeps sleeping. It does not
end a quiet channel, and in a quiet channel, which asks nobody, it wakes nobody.

### Refusals

A refusal is a `ChannelError::Forbidden`, which `caelumora_runtime::error::is_forbidden` recognises,
so that REST answers `403` rather than `500`. Like a refused tool call, it says what was refused,
not which rule refused it.

## Actors and envelopes

An **actor** is anything that sends and receives events in a channel.

*   **`Actor`** (`caelumora_runtime::actor`) is the trait of all actors. Its task is `async fn run`,
    which receives envelopes and sends its output through the transport it is given. `wakeup`,
    `cancel` and `stop` are synchronous, because they come from other tasks and must take effect at
    once. The trait is crate-internal (issue 147): only the channel controls its actors, after
    the operation that does it checked its permission (see
    [SECURITY.md](SECURITY.md#operations-that-check-themselves)). What an agent's model saw is not
    asked of its actor but read through the views (see
    [AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw)). The implementations are `AgentActor`,
    `UserActor`, `ApproverActor` and `ExternalAgentActor` (`caelumora_runtime::actor`).
*   **An actor's inbox** is an `EventReceiver` (`caelumora_runtime::transport`): `recv` waits for
    the next envelope, `recv_matching` for the next one of a kind - an agent waiting for approvals
    leaves the others queued - and `try_recv` takes what is queued already without waiting. `commit`
    acknowledges everything taken since the last one. Both kinds of agent take an envelope together
    with everything queued behind it (`actor::intake::take_in`, issue 222): every message heard is
    taken in, in order, and only the last request is answered, once - see
    [AGENT-TURN.md](AGENT-TURN.md#the-cognitive-cycle).
*   **Which actor runs an agent** is its personality's `kind` (issue 179): `internal` - the
    default - an `AgentActor`; `external` an `ExternalAgentActor`, for an agent that runs elsewhere
    and is reached through the personality's one `ExternalAgent` component. The channel decides it
    when it builds the actor (`Member::of`), which reads the personality once, as the agent may read
    it, and hands it to the actor to be built from; the agent is `ActorId::Agent` either way, and
    nothing else - the participants, the REST API, the moderator - tells the two apart.
*   **An external agent** has no turn and no tools. Its actor hears what others say, as every
    participant does - and, when it joins, what it may see of the channel's - and takes it in as an
    agent does (issues 196, 206): all of it into its history, with its own answers; what it hears of
    what others said - their words, by the rule every agent hears by (`Message::as_heard`, issue
    224) - as it passes the `PerceiveInProcessor`s of its personality, into its transcript. What it
    reads of the channel, its prompt providers read. When it is asked to respond - once for all it
    took in together, in the contexts of the last request, as an agent begins one turn (issue 222) -
    it gives the remote agent the prompt parts of its personality first, such as who is in the
    channel (`channel_members_prompt_provider` with `changes_only`, issue 204, see
    [AGENT-TURN.md](AGENT-TURN.md#who-is-in-the-channel)). Its `ContextProvider` assembles them, as
    an agent's does for a turn (issue 214) - by default `external_context_provider`, which gives
    each part on its own, with the component it came from, in the order of the configuration,
    positions ignored. The actor gives each part only when it is not what the remote agent was given
    last of it (`RemoteState::given_prompts`, stored), since the remote agent keeps its
    conversation; a part a failed send did not give is given with the next one, also when its
    provider has nothing new by then. After a restart only what changed is given; when the remote
    agent opens another conversation, all of it again. A2A has no system message: the parts are text
    of the user's message too, ahead of what was said. Then it gives the remote agent its
    transcript - each message a part of its own, marked with its sender as the component's profile
    says (`sender_format`, issue 135; `Said` keeps the sender beside it, for a protocol that can
    carry one) - which then starts anew (`Transcript::start_anew`, issue 205): the transcript's
    active path is what the remote agent has not heard yet - what a failed send did not give is
    given with the next one, and a restart gives nothing twice - and what each call gave stays as a
    branch from its root. It publishes what comes back: pieces as `PartialMessageGenerated`, the
    answer as `TurnFinished`, a task the remote agent failed as `TurnGivenUp`. A question
    back from the remote agent is its answer; the next request continues that task, which is done as
    that request's work from then on, whatever the remote agent tells of it (issue 213). A cancel
    cancels the remote task and publishes what came so far - unless the cancel came from lowering
    the channel, which gives the answer up (issue 212, see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md)); a stop cancels it - and every other open task of the
    agent - and publishes nothing. Each request is a piece of work (`Work`), as a turn is - whose
    `TurnState` holds one: bound by the channel's classification as it is when it begins, and
    classified as what it has seen - all that was given and what the remote agent declares to bring
    in.
*   **An external agent speaks on its own** when a task outlives the request it was given for: the
    answer broke off before the task ended, or the channel was restarted meanwhile - also by a
    crash: a task is stored as soon as the remote agent took it on, with what it was given for it. A
    remote agent that ends a task when its client goes away - Hermes Agent does, as failed with
    "[client disconnected]" - has it told as failed after the restart. A request whose answer broke
    off - its stream ended or failed - before its task ended is answered with nothing rather than
    cut off: what came so far is kept with the open task (`OpenTask::unsaid`) and said once, with
    the rest, when the task ends - not twice if the remote agent gives its whole result again (issue
    195). Its actor follows every task left working (`ExternalAgent::resume`) beside the channel's
    events, once per run, and says what came of it on its own account. An open task is the external
    agent's counterpart of a turn (issue 211): it keeps the work of the request that began it, or
    took it up again (`OpenTask::work`) - its contexts, its bound and what it has seen - and is
    followed, cancelled and told of in them, classified so (issue 212) - with the authority of
    whoever asked, as a turn acts with its request's, and in the context of the answer to that
    request (issue 45). A task stored before its work was kept acts as the agent itself; one whose
    channel was lowered since it began is given up. It wakes nobody and does not end a quiet
    channel: sleeping agents hear it all the same, and only an agent it mentions is woken, to answer
    it (issue 199). Only the end of a task is said: a failed task as `TurnGivenUp`, keeping nothing
    of what came before, and a stream that breaks before the task ends says nothing - the task stays
    open and is followed again on the next run. While it answers a request, what its other tasks
    send waits. What it heard and said is all that is known of it, kept in its `AgentChannelState`
    as every agent's, with what ties it to the remote side (`RemoteState`, issue 206; see
    [STORAGE.md](STORAGE.md)).
*   **`ActorId`** identifies an actor: a `User`, an `Agent` or an `Approver`, or `System` - the
    runtime itself, set only deliberately through `SecurityContext::system()`.
*   **`Envelope`** is the unit of communication: a timestamp, a `SecurityContext`, an `EventContext`
    and the `ActorEvent` itself.

## The channel and actor runtime

A `ChannelLoop` is built with a `ChannelRuntime`, not with the `RuntimeManager` itself. It carries
what everything in the channel shares:

*   the process-wide services of the `RuntimeManager`, each by an accessor of its own - the stores,
    the security manager, the controllers - and never the `RuntimeManager` itself, so that what a
    channel can reach is what its runtime offers (issue 191);
*   the user commands of the session: a `CommandExecutor`, built once per session by
    `RuntimeManager::new_command_executor`;
*   a handle to the channel. It is a `Weak`, set once by `ChannelLoop::new` when the channel exists:
    the channel owns its actors, and a strong reference back would keep a stopped channel alive.

The `ChannelRuntime` is the only source of `ActorRuntime`s, so every actor belongs to exactly one
channel.

An `ActorRuntime` is made for one actor (`ChannelRuntime::new_actor_runtime(actor, session,
channel)`) and hands out only what concerns that actor (issue 69):

*   the services every actor needs: `security_manager`, `secret_manager`, `component_registry`,
    `command_executor` - the model is not among them, an agent has it as a component (issue 130);
*   its own agent state (`agent_state`) and its state in the channel (`load_channel_state` /
    `store_channel_state`, of either kind of agent, issue 206);
*   its own event subscription (`subscribe_channel` - its channel, nothing else) and announcement
    (`announce_channel_state_changed`);
*   the loop of its channel (`channel_loop`, `pub(crate)`), and a stored session as a given context
    may read it (`load_session`);
*   the views of its turns (`views`, issue 163), which read the session's other channels as the
    confidentiality rules allow (see [MEMORY.md](MEMORY.md#reading-other-channels)).

The full `ChannelLoop` and `SessionLoop` are `pub(crate)`, so components from other crates cannot
reach them; within the runtime, the user actor needs the channel for commands, and the views need
both.

Components are not given their actor's `ActorRuntime`, only what their `ComponentContext` holds
([128](../issues/128-actor-runtime-in-components.md)); see [COMPONENTS.md](COMPONENTS.md).

## Moderators

A moderator decides how messages are routed and who is asked to respond. It is a component of the
channel (issue 174): a channel has components (`Channel::components`, from
`ChannelTemplate.components` or the request), which `ChannelLoop::new` builds as an agent builds its
own - in the name of the channel's owner - and takes the one `Moderator` of. `Moderator` and
`ResponderPolicy` are facets of which a channel has exactly one, each with a default (see
[COMPONENTS.md](COMPONENTS.md#facets-of-which-there-is-exactly-one)):

```yaml
channels:
- name: main
  components:
  - id: responders
    kind: mentions_only_policy
```

The built-in moderator, `policy_moderator` (the default), routes the way every channel routes:

*   every message said (`CompleteMessageGenerated`) goes, as it was said, to every other
    participant: to the users always, to the agents if there is something to hear in it
    (`Message::is_heard`, issues 224, 258) - each agent takes in its words itself;
*   whom to ask to respond is decided when an answer is complete (issue 258): for a message said
    outside of any turn - a user's, one an external agent says unasked - as it is said; for the
    messages of a turn at its end, `TurnFinished`, which goes to the users too, who learn so that
    an answer ended even when it said nothing (issue 225). The decision follows from the messages
    - whether a turn is theirs - never from the kind of their sender;
*   how an answer comes about - `ResponseStarted`, partial messages, intermediate ones, plan steps -
    goes to the users, not to the agents (issue 220); so does where an agent stands,
    `StateChanged` (issue 243), and how a turn was given up, `TurnGivenUp`;
*   approvals and announced commands go to whom they concern;
*   every agent a message `@`-mentions is asked to respond;
*   a chain between agents ends at a limit (see below).

Whom it asks when a message mentions no agent is its slot, the channel's `ResponderPolicy`, which it
asks for when it is built (`caelumora-runtime/src/moderator/`). The policy is told who wrote
the message and which agents could answer. A new way of choosing who answers is a new policy:

| Policy | Who responds to a message without a mention |
|---|---|
| `broadcast_policy` | every agent |
| `round_robin_policy` (default) | the agents take turns, in the order of their ids; a mention does not take a turn |
| `random_policy` | one agent, chosen at random |
| `mentions_only_policy` | nobody: agents respond only when someone mentions them |
| `user_driven_policy` | every agent if a user wrote, nobody if an agent did |

Under the first three, an agent's answer makes other agents answer too: the agents can discuss with
each other. Under `user_driven_policy` (issue 175) the agents answer when a person writes and are
quiet in between: an agent's message is still delivered to the other agents, into their transcripts,
but starts no turn. Loops come from asking to respond, not from delivering.

What a moderator decides is all a participant is told: `Moderator::route` returns whom an event is
delivered to and as what - the event itself, or one of the moderator's own, such as a request to
respond - and the channel delivers exactly that. A user sees what is routed to their actor (issue
220); the channel has no broadcast of its own besides. Nothing goes back to the actor that sent an
event: an agent takes its own messages into its history and transcript itself, and a user's actor
shows what its user says to those who follow it itself (`UserActor::send`).

### Chains between agents

An agent's message that makes other agents respond - because it mentions them, or because the policy
chooses them - is a *hop* of a chain between agents (issue 45). `EventContext::agent_hops` counts
the hops since the last message of a user: a user's actor starts every envelope at 0, `next` keeps
the count, so an agent's answer carries the count of the request it answers, and the
`policy_moderator` sets it one higher on the `ResponseRequested` it sends for an agent's message.
Once that would exceed the moderator's parameter `max_agent_hops` (default 3), it asks nobody - nor
its policy, so a rotation is left as it is. The message is still delivered, and the moderator says
that the chain was cut off (issue 200): an `ActorEvent::ChainCutOff` in the name of
`ActorId::System`, with the last message of the response, the limit and the agents it mentioned
(`not_asked`, empty if none). It goes to the users of the channel and to the agents involved - the
sender of the response and those it mentioned - but not to the other agents or the approvers. It is
never written to the history, and an agent takes no note of it. Since it is sent in another name,
the channel delivers it to the sender, too: what it holds back from the sender is only what the
sender sent itself. The users see it as everything else, through their actors (issue 220). With
`max_agent_hops: 0`, agents answer only users.

A user's message starts a new chain. What an external agent says unasked when a long task ends
continues the context of its answer to the request that began the task (`Work::event_context` of
the open task), so it is as many hops away as that answer was.

`max_route_decisions` in the `ChannelLoop` is a different thing: a safety net against faulty
routing, counted in routing steps (`span_depth`), not a rule for users.

The REST API returns each channel's `moderator` and `responder_policy` - their kinds as configured,
`null` for a preconfigured component (`ComponentRegistry::kind_of`) - and the web UI explains the
policy to the user (see [WEB-UI.md](WEB-UI.md)); the TUI prints both when the chat starts.

### `@`-Mentions

A user can address participants directly with `@handle` (e.g. `@sassy` for "Sassy"). A handle is the
participant's name transliterated to ASCII (`any_ascii`, issue 177) and lowercased, with every run
of other characters collapsed into `-` (`caelumora_model::mention`): "Zoë" is `@zoe`. A mention is
normalised alike, so `@Zoë` addresses her too. Unknown handles stay plain text, and text in
backticks is ignored.

The `ChannelLoop` resolves mentions once, as a message enters the channel, and stores the resolved
`ActorId`s in `Message::mentions`. Nothing downstream parses the text again, so a later rename
cannot change whom a past message addressed.

A mention changes who is asked to respond, not who receives the message: every participant still
gets it, but every moderator (through the shared `ModeratorActors::mentioned_responders`) sends
`ResponseRequested` to exactly the mentioned agents instead of applying its own policy. Mentioned
agents that are asleep are woken first (see *Waking*).

Mentions written by agents are routed as well: an agent can hand a question to another
(`@researcher, can you look this up?`). The limit on chains between agents keeps two agents from
addressing each other indefinitely; an agent that mentions itself or an author of the message is
not asked (`mentioned_responders` leaves out the senders).

The REST API lists each channel's `members` in no particular order, each with its handle (`null` for
participants that cannot be mentioned, such as approvers), which the web UI uses for `@` completion
and highlighting.
