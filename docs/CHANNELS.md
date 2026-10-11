# Sessions, Channels and Actors

Caelumora uses an actor-based model: people and agents take part in **channels** as **actors**,
which exchange **envelopes**, and the channels of one interaction form a **session**. This document
describes how sessions and channels are made, run and changed, how actors act in them, and how
moderators decide who responds. What an agent does in its turn is in [AGENT-TURN.md](AGENT-TURN.md),
how the channel's state is stored in [STORAGE.md](STORAGE.md), and how clients follow a channel in
[REST-SERVER.md](REST-SERVER.md#streams).

## Sessions

A **session** is the container of one interaction. Its `SessionLoop` runs its channels.

Every channel is a document of its own; the session lists its channels by id, in order. Creating a
session stores its channels with it, deleting it keeps them - hidden with it, since a channel is
reached only through its session -, purging it removes them. A session is loaded in the runtime's
name, since starting a session the caller may read must not fail on a channel the caller is not in.

What lies in a session's scope - its personalities, session templates and agents, those of its
channels' scopes, its workspace if made for it, the artifacts published in it and not kept - goes
with it (`caelumora-runtime/src/scoped.rs`): deleting the session deletes them, restoring it
restores those deleted with it - not one deleted by hand before, nor one whose name was taken
meanwhile -, purging it purges them. That is done in the runtime's name, as a consequence of what
was permitted.

That holds because what lies in a session's or a channel's scope is used only within it
(`Scope::admits`): an agent of it takes part only in that session's channels, or in that channel,
and a personality of it makes agents only within it. What lies in a user's scope or everywhere is
used wherever the permissions allow: that scope only says where its name is found. A session
template lies only in a user's scope or everywhere; one of a session's or a channel's would make
sessions it can never lie in, and is not stored.

Every session has a privacy policy: whom it lets read what was said in it from other sessions. It is
taken over from its template (private unless the template says otherwise), changed only by the
session's owner, and shown to everyone who takes part. What it decides is described in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#reading-another-channel).

### Creating and deleting channels

Channels can be created in, deleted from and restored to a running session
(`SessionLoop::create_channel`, `delete_channel`, `restore_channel`). Each is stored first, and only
then is the running session changed and the change announced:

*   **Creating** stores the channel (`CreateChannel`) and adds it to the session, which asks
    `UpdateSession`, so that only the owner may. A channel's name is unique in its session: the
    store refuses one the session has, when a channel is created, renamed or restored.
*   **Deleting** takes the channel out of the session and marks it deleted (`DeleteChannel`); the
    channel and its state are kept. What lies in its scope is deleted with it.
*   **Restoring** asks `UpdateSession` and its participants' clearance, as creating does, lists the
    channel again and starts it from its stored state. What was deleted with it comes back, but for
    one whose name was taken meanwhile.
*   **Purging** is not a channel's: a deleted channel is removed for good with its session.

A deleted channel stops its actors before its task is aborted: every actor is told to `stop` and
given up to `STOP_GRACE` to end, in which an external agent cancels its open remote tasks and every
actor stores its state.

### Stopping an actor

`Actor::stop` is synchronous and goes past the events, like `cancel`: it takes effect at once, not
after what is queued. Every actor ends its own task when stopped. The channel's run loop alone owns
the actors' tasks, so that they end with the channel's; whoever stopped an actor waits on its task
separately. Removing a participant and deleting the channel use the same grace: an actor that has
not ended after `STOP_GRACE` is aborted. A tool that hangs does not keep an agent: a stop interrupts
it, and the agent gives its turn up within the grace, the call recorded as having run.

### One writer per document

Each document has one writer, which holds a lock while it changes and stores it: the
`SessionController` for the session, the `ChannelController` for its channel. A change - a new
name, say - is stored first, where it may still be refused, and only then taken over and announced.
The controllers are described in [STORAGE.md](STORAGE.md#controllers).

## Channels

A **channel** is one conversation within a session, such as a chat room. Its `ChannelLoop`:

*   manages the lifecycle of the **actors** taking part in it;
*   routes **envelopes** between them;
*   applies a **moderator**, which decides who is asked to respond.

### Building a channel

Every channel is built by `Channel::build`, from a template or on request. It adds an approver if
the channel has none, and lets its members read it: an entry in its access list naming the group of
its participants, which follows whoever takes part at the time and is kept by the
`ChannelController` as someone joins or leaves (see [SECURITY.md](SECURITY.md#groups)).

The participants of a requested channel come from the session's participants, the creator always
among them. Its moderator and classification default as in a template (see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md) and [Moderators](#moderators)).

A request that cannot be met is refused before anything is stored: no name, a name the session
already has, someone who is not a participant of the session, or a classification the policy
refuses - the last asked for channels from a template too. Components that make no moderator are
found only when the channel is built, after it was stored; it is then taken out of the session and
deleted again.

### Its language

A channel may be held in a language, a BCP 47 tag (`Language`). A channel is held in the language
its creator prefers unless the request or its template names another - a template for a channel
whose purpose comes with its language, such as a German quiz; one whose creator prefers none is held
in none. Whoever may change the channel changes it or takes it back. Its agents are told by a prompt
part (see [AGENT-TURN.md](AGENT-TURN.md#the-channels-language)); nothing said before is translated.

### What it is for

A session and a channel may say what they are for: a `summary`, one line seen at a glance, and a
`briefing`, what an agent should know before it works there - conventions, context, what to do and
what not, like an `AGENTS.md`. Both come from the template and are changed by whoever may change the
place. Their types bound them (`caelumora_model::value::text`): a briefing is in the context of
every agent there on every turn. They are kept exactly as written; trimming is the input's.

A session's descriptions are public, as its name is: everybody in the session reads them, and every
agent in each of its channels, external ones too. A channel's are part of the channel and classified
as it is now. Agents are told both by a prompt part (see
[AGENT-TURN.md](AGENT-TURN.md#what-the-session-and-channel-are-for)).

### Its workspace

A session may have one workspace attached, a tree of files of a user's shared by the session's
channels. A channel uses it where its switch says so (`Channel::uses_workspace`) and the workspace
and the channel flow both ways; attaching it shares nothing by itself, its access list decides who
may use it. How a session comes to its workspace and who reaches the files is in
[WORKSPACE.md](WORKSPACE.md#in-a-session).

### Running channels

`SessionLoop::run` runs every channel, those the session had and those created later.

*   Stopping the session aborts every channel; the commands started in its workspace shell are
    ended.
*   Deleting a channel aborts that channel's task; its state is kept until it is purged. A channel
    deleted while it waits to be started is never started.
*   A participant that cannot be run is left out when the channel starts, with a warning, and the
    channel runs with everyone else: a user or agent deleted since it joined, or an agent whose
    personality or components were withdrawn or deleted. A third party - the owner of a personality
    or a component - must not keep someone's channel from starting. Such an agent is still a
    participant; adding it to another channel is refused.

### Who may see a channel

A channel its viewer may not read (`GetChannel`: owner and participants) is answered as absent and
left out of lists and snapshots - the same as one that does not exist. The runtime decides that, not
the REST server: `SessionLoop::channel_for` returns a channel only to whoever may read it, and the
views answer the same way. What a client lists and offers comes from views the runtime builds for
one viewer; see [SECURITY.md](SECURITY.md).

## Acting in a channel

Whoever writes or approves does so in their own name. `ChannelLoop::user_actor(UserId)` returns
*that user's* actor, and every envelope is built by the actor itself, so sender, name and
`SecurityContext` follow from it. There is deliberately no way to reach "some" user actor. Someone
who is not a participant of a channel has no actor in it and is refused.

A frontend talks to the user's actor, never to the channel. The actor queues what its user writes,
approves or announces, and its task sends it on through the transport, like every actor's output; no
actor takes a way around it. The queue closes when the actor's task ends, so that sending to a
stopped channel fails instead of waiting.

The channel does not take an actor's word for whose name it speaks in. Each actor is given an
`OriginEventSender`, which passes on only an envelope that speaks as the actor: as its sender, as
the caller of its `SecurityContext`, and as the sender of every message it carries. Anything else
is refused and logged as an error - a bug of the actor. Whose rights the actor has and who asked it
may be someone else: an agent acts with its owner's rights and answers whoever asked it (see
[SECURITY.md](SECURITY.md#in-whose-name-an-agent-acts)). What the channel says itself - a
moderator's notice, a removal - does not come through an actor's sender.

**Nothing said is unsaid.** The channel keeps every message it admits in its history as it arrives,
the steps of a plan included, records where each step of a plan begins, and after a turn's messages
how it ended. A turn given up keeps what it said; only a message still streaming when it ended is
lost, never having been complete. How a turn ended is told by whoever had it, also by an agent
removed while it answers: the channel takes what a removed actor sends until its task has ended, and
records no end in anyone's name. A turn whose agent was aborted after the grace, saying nothing,
stays without an end: nobody ended it.

**The channel decides what enters it.** Every event an actor sends is admitted or refused by the
channel's loop, one after another, as it takes it in (`ChannelLoop::admission`) - against the
channel as it is at that moment, so that nothing changes between a check and the entry. An actor
asks nothing of the policy for what it sends; it looks only at what it can see at once. The channel
asks, in the sender's name, first whether the sender may act so, then whether what it carries may
flow in:

| What enters | What the channel asks |
|---|---|
| a message | it speaks as the actor that acts; each part, the message and its tool results, may flow in as classified (`may_publish`) |
| `MessageModified`, `MessageRevoked` | the message is in the history; `ModifyMessage`, `RevokeMessage` of its author; it can be changed so; a modification may flow in |
| `MessageReclassified` | the message is in the history; `Reclassify` of its author; never above the channel; it can be reclassified |
| `RetryRequested` | `RetryMessage` of the channel; a message an agent said, not taken back, or an agent taking part |
| `QuestionAnswered`, `QuestionDeclined` | `AnswerQuestion` of the channel - whether the sender was asked is the agent's to check |
| `CommandRequested`, `NudgeRequested` | sent by a user, naming an agent (or, for a nudge, anyone else) taking part |

A message is admitted as it arrives, so one said after the channel was lowered below its turn is
refused. What is refused is written nowhere and delivered to nobody. The channel tells its sender
alone why, in the system's name (`MessageRejected`, with a `Rejection`), never in the history. A
user's actor shows what its user sends before it forwards it, so that the refusal always follows
what it refuses (see [WEB-UI.md](WEB-UI.md)). What an agent said that the channel refused stays in
its own transcript only.

Waking the channel for a message is the actor's job, too: it sends a `WakeupRequested` ahead of
every message. Neither a frontend nor the channel decides that by itself.

User commands (`/help`, `/wakeup`, `/nudge`) are run by the user's actor in the same way; a command
an agent carries out itself is sent to that agent, which carries it out in a turn of its own. See
[COMMANDS.md](COMMANDS.md).

## Changing what was said

What was said can be changed by its author, but never rewritten: a message is *modified* - it reads
differently from now on, at the place it has - or *taken back*. Nothing forks: the others saw the
message, and what they said in reply stays.

*   **A user** changes their message through their actor, as they write. The actor looks the message
    up in the channel's history, so that the user learns at once of one that is not there or was
    taken back. A modified message speaks as its author, so only the author's actor can send it.
*   **The channel admits it** as it admits a message (above): the message is in its history, its
    author may change it, it can be changed so as it stands - not taken back, modified by its own
    sender in its own role - and a modification may flow in. It records the change in its history.
*   **Everyone who heard it follows.** The moderator routes the change to every user and agent but
    its sender and asks nobody to respond. An agent takes it in between turns, in order with what it
    hears; a modification passes its perceive processors as anything it hears. A turn under way ends
    as it began; the next is given what stands. An agent that never held the message records
    nothing. An external agent tells its remote agent instead, since A2A has no way to change what
    was sent (see [AGENT-TURN.md](AGENT-TURN.md#changes-to-a-message)). That only asks: the remote
    agent decides whether the old version stops acting.
*   **Reclassifying** a message goes the same way. The policy decides who may - its author, or the
    channel's owner for someone else's - never above the channel, and the next turns of everyone who
    heard it are labelled by it as it stands (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reclassification)).
*   **Taking back is not erasing.** The message stays in every participant's record, in the
    contexts of what was answered to it, and with a remote agent that was given it (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#messages)).

### Answering again

Whoever takes part may ask an agent to answer again instead of one of its messages - its last
answer and nothing else, since a new answer cannot be placed above what answered the old one without
forking - or instead of its answer still under way, or anew after its last turn, given up because
it failed or the channel was lowered, while nothing followed it. It is a request to the agent, not
an operation of the channel: `RetryRequested`, naming a message or a turn.

*   **A user** asks through their actor, which refuses at once, without an event, what cannot
    succeed: a message not there, taken back or no agent's, a turn that is neither the agent's turn
    under way nor its last turn given up that may be answered again (`ChannelLoop::retryable_turn`).
*   **The channel** admits it (`RetryMessage` - as cancelling and waking, whoever may read the
    channel), and the moderator routes it to the agent alone, which the channel wakes first. A turn
    under way is ended as the request is delivered (`Actor::retry_turn`), decided in one step under
    the turn's lock. Whether a retry is possible is decided before anything is interrupted: a
    request about an older message never ends the answer under way.
*   **The agent**, its turn under way asked for again, ends the step at once, as on a cancel, and
    gives the turn up as `Retried`: what it said stays said, followed by its end, so its model sees
    the calls that ran and nothing has to be taken back. A cancel asked before wins: the turn is
    taken over instead.
*   **The agent judges the requests** between turns, with what it hears:
    *   the request of the turn it gave up for it is accepted;
    *   a request of its last turn, given up because it failed or the channel was lowered, with
        nothing after it, is accepted and answered anew after it - nothing taken back; otherwise
        it is rejected;
    *   of the requests of a message, the last is judged against the conversation as it stands, the
        others rejected. It is accepted only if the message is the agent's last answer, as its own
        transcript says: then the agent takes back the message and the rest of its turn, in its
        own state and in the channel. Otherwise it says why, as one of a known few reasons
        (`RetryRefused`), which a user interface words itself.

    A retry accepted and a request to respond in the same batch are one turn, in the request's
    contexts; its model is given the conversation as it stands, without what was replaced.
*   **An external agent** rejects every request to answer again: its remote agent keeps what it was
    given, and nothing here takes the answer back from it.

Whether a retry was accepted is told to the users and never written to the history. Whether a viewer
may ask is part of each member's snapshot - the viewer's `RetryMessage`, of an agent whose kind
answers again - so a client offers it only where it is permitted and can succeed.

## Participants

Participants of the session can be added to and removed from a running channel. Changing the
participants is an operation of the channel, not of the session (`ChannelLoop::add_participant`,
`remove_participant`). Like renaming, the change is made by the channel's one writer, the
`ChannelController`: it recomputes the channel's security attributes from its participants, stores
the channel (`UpdateChannel` - only the owner) and announces who joined or left. Events only
announce; nothing changes a channel because an event arrived.

Adding is asked first as `UpdateChannel`, before anything of the newcomer is looked at, and then as
`cleared` with the channel's classification, where a policy can hold the channel's level against the
newcomer (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

A removed participant is kept as a former one, with who removed it and when, not among those who
take part now - so nothing that reads them changes. The views reach former members, so what an agent
said and saw stays readable for whoever may read its context.

Deleting an agent makes it leave every channel it takes part in, running or not, as a former
participant - removed in the runtime's name, since deleting the agent was permitted. Channels of
deleted sessions keep it, and leave it out if they are restored, as any participant that no longer
exists. A deleted agent keeps its name in the views, so that what it said keeps its sender.
Restoring it brings it back into no channel, and is refused if another agent took its name in its
scope meanwhile. Purging it removes its document and its state, its own and in every channel. An
agent of a session's or a channel's scope goes with that session or channel (see
[Sessions](#sessions)).

### How much history a newcomer sees

Whoever adds a participant decides how much of the history so far they may see: all (the default),
none, the last n messages or the last n days (`HistoryAccess`). It is fixed at once as the message
the participant's history starts at, so "the last three days" does not move day by day. The channel
and its views apply it wherever the history is read for a participant: over REST, in the snapshot,
for `read_history` (see [MEMORY.md](MEMORY.md)) and for what a joining actor is given.

*   A **joining actor** is given the visible part and takes it in as what it hears: an agent keeps
    all of it in its history and what passes its perceive processors in its transcript. Users and
    approvers keep nothing.
*   A **rejoining actor** keeps only what was said after the last message it has. The history is
    only ever appended to, so older messages that a wider grant makes visible would land after the
    agent's own later answers; the agent reaches them through `read_history`, and what it keeps
    stays in the order things were said.

Notes stay visible to everyone in the channel: they are written on paper, not part of the
conversation.

### What changes in the running channel

The channel starts the new actor's task, tells the moderator and rebuilds the mentions; a handle
that becomes ambiguous is logged.

A removed actor is stopped at once - an agent without finishing its turn, which it gives up and says
so - and its state is kept, for the record and for a later return. The approver answers a leaving
agent's pending approvals as denied, so that nothing keeps waiting, and decides the others again,
since who may approve them changed. The approver itself cannot be removed.

A participant added again while its old actor still ends waits for it, at most a little past the
grace, and is refused as still leaving after that. The new actor thus loads the state as the old one
left it, and never runs next to it. What the old actor sent before it ended is routed before the new
one starts under the same id. An actor removed before the run loop started it is never started.

### How others learn of it

A leave is announced on the event bus like every change of membership. In the channel itself, the
removal is announced with `ParticipantRemoved`, in the name of whoever removed the participant -
`System` if the runtime did - once the actor's task has ended, and at the latest before an actor
starts under the same id. Until then the channel takes what the actor still sends; after it, nothing
of the actor enters the channel. That is how a frontend ends what an actor aborted without a word
left under way. Every moderator forwards it to the users only, and it is never stored.

A removed user's live stream of the channel ends because it is theirs: it is followed through their
actor, which ends when it is stopped, so no one has to listen for the announcement.

Every channel response carries `can_change`, so that a client offers adding and removing
participants only where the user may (see [WEB-UI.md](WEB-UI.md)).

## Acting on the channel as a whole

Cancelling and waking act on the channel as a whole. They ask the `SecurityManager` first, one
decision for the whole channel rather than one per actor.

### Cancelling

`ChannelLoop::cancel` (`CancelChannel`) ends the current turn of every actor - the web UI's stop
button. It is an operation on the channel, like renaming or deleting it, not an act of taking part:
the policy decides who may, and nobody has to be a participant for it.

It is not a kill. The agent takes its turn over, so that observers stop waiting even when the turn
said nothing, and returns to a normal state with its history intact. But nothing under way is waited
for: a model call in flight is dropped at once, a turn waiting for an approval ends without one and
withdraws its requests, and a running tool call is answered as interrupted, no further call of the
step started (see [TOOLS.md](TOOLS.md#running-tool-calls)). A stop works
the same way, and wins over a cancel.

Lowering a channel's classification cancels its turns in the same way; each agent then gives its
turn up instead of taking it over (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

Afterwards the channel is *quiet* until it is woken. What is said is still delivered, so that every
agent's transcript keeps up with the history, but nobody is asked to respond to it.

`ChannelLoop::cancel_actor` ends the turn of one actor alone, under the same permission, asked
before the actor is looked up, so that who may not learns nothing of who is in the channel. The
channel does not become quiet: being quiet is the channel's state, not one actor's. The others go
on, and the answer the cancelled turn is taken over with reaches everyone as any answer does.

`ChannelLoop::interrupt_tool_call` ends less: one tool call of an actor's turn, while it runs or
waits to run. The call is answered as interrupted, and the turn goes on with its next step, so that
the model can deal with it. It is cancelling a part of a turn, under the same permission, asked
first. See [TOOLS.md](TOOLS.md#running-tool-calls).

### Waking

A quiet channel is woken in one of two ways:

*   **Deliberately**: `ChannelLoop::wakeup` (`WakeChannel`), as `/wakeup` does; `wakeup_actor`
    wakes a single actor under the same permission. The channel moves a sleeping agent to `Idle`
    itself; the agent announces it (see [AGENT-TURN.md](AGENT-TURN.md#the-cognitive-cycle)).
*   **Because something is on its way**: a `WakeupRequested` that a user's actor sends ahead of its
    next message. It asks nothing: it follows from an action that was already permitted, and a user
    allowed to write but not to wake would otherwise be unable to write.

A single agent is also woken when the moderator asks it to respond: a sleeping agent ignores a
request to respond, so the channel wakes everyone a decision asks before it delivers anything of
that decision. The policies never choose a sleeping agent, so this wakes only agents a message
mentions; an agent that is not addressed keeps sleeping. It does not end a quiet channel, and in a
quiet channel, which asks nobody, it wakes nobody - unless a user nudges the agent.

### Nudging someone

A user asks someone in the channel - an agent, another user, any participant - to respond to the
conversation as it stands, without saying anything, with `/nudge <participant>`: what an empty
`@sassy?` would do, without a message in the history. The channel admits it from a user, for someone
else taking part, and the moderator turns it into the request to respond to them alone, as if a
message of the user had mentioned them - no policy consulted, no chain between agents counted.

*   **What it asks of whom.** An agent answers it in a turn, with the authority of the user who
    nudged, as after a message of theirs. A user is shown who asks them to respond - the only way a
    user is asked. An approver takes no such request in.
*   **No permission of its own.** Whoever may write in the channel can ask anyone to respond by
    mentioning them; the nudge asks the same, so it asks nothing of the policy. Nudging oneself is
    refused: nobody would be asked.
*   **Deliberate, like a `/wakeup` of an agent.** The channel ends being quiet as it routes the
    nudge, and wakes whom it asks. No `WakeupRequested` goes ahead of it: that would wake every
    agent.
*   **Not into a second turn.** An agent in a turn would take the request in after it and answer
    twice, so the nudge is refused while it responds.
*   **Nothing remains of it** but the answer: the history keeps no entry, nobody but the one asked
    is shown the request, and the command answers the issuer alone.

### Refusals

A refusal is a `ChannelError::Forbidden`, which REST answers with `403`. Like a refused tool call,
it says what was refused, not which rule refused it.

## Actors and envelopes

An **actor** is anything that sends and receives events in a channel.

*   **`Actor`** (`caelumora_runtime::actor`) is the trait of all actors: `AgentActor`, `UserActor`,
    `ApproverActor` and `ExternalAgentActor`. Its task, `run`, receives envelopes and sends its
    output through the transport it is given. `wakeup`, `cancel` and `stop` are synchronous, because
    they come from other tasks and must take effect at once. The trait is crate-internal: only the
    channel controls its actors, after the operation that does it checked its permission (see
    [SECURITY.md](SECURITY.md#operations-that-check-themselves)). What an agent's model saw is read
    through the views, not asked of its actor (see
    [AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw)).
*   **An actor's inbox** (`EventReceiver`) can wait for the next envelope of a kind, leaving the
    others queued - as an agent waiting for approvals does - and acknowledges what was taken. Both
    kinds of agent take an envelope together with everything queued behind it: every message heard
    is taken in, in order, and only the last request is answered, once (see
    [AGENT-TURN.md](AGENT-TURN.md#the-cognitive-cycle)).
*   **Which actor runs an agent** is its personality's `kind`: `internal` - the default - an
    `AgentActor`; `external` an `ExternalAgentActor`, for an agent that runs elsewhere and is
    reached through the personality's one `ExternalAgent` component. The channel decides it when it
    builds the actor, reading the personality as the agent may read it; the agent is
    `ActorId::Agent` either way, and nothing else - the participants, the REST API, the moderator -
    tells the two apart.
*   **`ActorId`** identifies an actor: a `User`, an `Agent` or an `Approver`, or `System` - the
    runtime itself, set only deliberately.
*   **`Envelope`** is the unit of communication: a timestamp, a `SecurityContext`, an `EventContext`
    and the `ActorEvent` itself.

### External agents

An external agent has no turn and no tools here. It hears what others say and takes it in as an
agent does: all of it into its history, what it hears through its perceive processors into its
transcript. When it is asked to respond - once for all it took in together, in the contexts of the
last request - it gives its remote agent:

*   **the prompt parts** its `ContextProvider` assembles, as an agent's does for a turn - by default
    each part on its own, in the order of the configuration. The remote agent keeps its
    conversation, so a part is given only when it is not what was given last of it; after a restart
    only what changed is given, and to a new remote conversation all of it again. A2A has no system
    message: the parts are text of the user's message;
*   **then its transcript**, each message marked with its sender as its profile says, which then
    starts anew: what the remote agent has not heard yet is what is given next, what a failed send
    did not give is given with the next one, and a restart gives nothing twice.

It publishes what comes back: pieces as partial messages, the answer as the end of the answer, a
task the remote agent failed as given up. A question back from the remote agent is its answer; the
next request continues that task, which is then done as that request's work. A cancel cancels the
remote task and publishes what came so far - unless it came from lowering the channel, which gives
the answer up; a stop cancels every open task and publishes nothing. Each request is a piece of
work, as a turn is: bound by the channel's classification as it begins, and classified as what it
has seen - all that was given and what the remote agent declares to bring in.

**An external agent speaks on its own** when a task outlives the request it was given for: the
answer broke off before the task ended, or the channel was restarted meanwhile - a task is stored as
soon as the remote agent took it on. An answer that broke off is answered with nothing rather than
cut off: what came so far is said once, with the rest, when the task ends. The actor follows every
task left working beside the channel's events. An open task is the external agent's counterpart of a
turn: it keeps the work of the request that began it, and is followed, cancelled and told of in it -
as asked by whoever asked, with its owner's rights, and in the context of the answer to that
request. One whose channel was lowered since it began is given up. It wakes nobody and does not end
a quiet channel; only an agent it mentions is woken, to answer it. Only the end of a task is said: a
failed task keeps nothing of what came before, and a stream that breaks before the task ends says
nothing - the task stays open and is followed again on the next run. While it answers a request,
what its other tasks send waits.

What an external agent heard and said is kept in its `AgentChannelState` as every agent's, with
what ties it to the remote side (see [STORAGE.md](STORAGE.md)).

## The channel and actor runtime

A `ChannelLoop` is built with a `ChannelRuntime`, not with the `RuntimeManager` itself. It carries
what everything in the channel shares:

*   the process-wide services, each by an accessor of its own, and never the `RuntimeManager`
    itself, so that what a channel can reach is what its runtime offers;
*   the user commands of the session, built once per session;
*   a handle to the channel - a `Weak`: the channel owns its actors, and a strong reference back
    would keep a stopped channel alive.

The `ChannelRuntime` is the only source of `ActorRuntime`s, so every actor belongs to exactly one
channel. An `ActorRuntime` is made for one actor and hands out only what concerns that actor: the
services every actor needs - the model is not among them, an agent has it as a component - and
those its components may use; its own agent state and its state in the channel; its own event
subscription, for its channel and nothing else; the loop of its channel; and the views of its turns,
which read the session's other channels as the confidentiality rules allow (see
[MEMORY.md](MEMORY.md#reading-other-channels)).

The full `ChannelLoop` and `SessionLoop` are `pub(crate)`, so components from other crates cannot
reach them. Components are not given their actor's `ActorRuntime` either, only what their
`ComponentContext` holds; see [COMPONENTS.md](COMPONENTS.md).

## Moderators

A moderator decides how messages are routed and who is asked to respond. It is a component of the
channel: a channel has components, from its template or the request, which the channel builds as an
agent builds its own - in the name of the channel's owner. `Moderator` and `ResponderPolicy` are
facets of which a channel has exactly one, each with a default (see
[COMPONENTS.md](COMPONENTS.md#facets-of-which-there-is-exactly-one)):

```yaml
channels:
- name: main
  components:
  - id: responders
    kind: mentions_only_policy
```

The built-in moderator, `policy_moderator` (the default), routes the way every channel routes:

*   every message said goes, as it was said, to every other participant: to the users always, to the
    agents if there is something to hear in it - each agent takes in its words itself;
*   whom to ask to respond is decided when an answer is complete: for a message said outside of any
    turn as it is said, for the messages of a turn at its end. The decision follows from the
    messages - whether a turn is theirs - never from the kind of their sender;
*   how an answer comes about - its start, partial and intermediate messages, plan steps - where an
    agent stands, and how a turn ended go to the users, not to the agents;
*   approvals and commands go to whom they concern: a request for a command to the agent it names,
    that it began to the issuer, its answer to the issuer alone if private, else to every user;
*   every agent a message `@`-mentions is asked to respond;
*   a chain between agents ends at a limit (see below).

Whom it asks when a message mentions no agent is the channel's `ResponderPolicy`, which is told who
wrote the message and which agents could answer. A new way of choosing who answers is a new policy:

| Policy | Who responds to a message without a mention |
|---|---|
| `broadcast_policy` | every agent |
| `round_robin_policy` (default) | the agents take turns, in the order of their ids; a mention does not take a turn |
| `random_policy` | one agent, chosen at random |
| `mentions_only_policy` | nobody: agents respond only when someone mentions them |
| `user_driven_policy` | every agent if a user wrote, nobody if an agent did |

Under the first three, an agent's answer makes other agents answer too: the agents can discuss with
each other. Under `user_driven_policy` the agents answer when a person writes and are quiet in
between: an agent's message is still delivered to the other agents, but starts no turn. Loops come
from asking to respond, not from delivering.

What a moderator decides is all a participant is told: `Moderator::route` returns whom an event is
delivered to and as what, and the channel delivers exactly that; it has no broadcast of its own.
Nothing goes back to the actor that sent an event but why it was refused: an agent takes its own
messages into its history and transcript itself, and a user's actor shows what its user says itself.

### Chains between agents

An agent's message that makes other agents respond - because it mentions them, or because the policy
chooses them - is a *hop* of a chain between agents. The hops since the last message of a user are
counted with the envelope (`EventContext::agent_hops`); an agent's answer carries the count of the
request it answers. Once a request would exceed the moderator's `max_agent_hops` (default 3), it
asks nobody - nor its policy, so a rotation is left as it is. The message is still delivered, and
the moderator says that the chain was cut off (`ChainCutOff`, in the system's name), to the users
and to the agents involved - the sender and those it mentioned - never to the history. Since it is
sent in another name, the channel delivers it to the sender too. With `max_agent_hops: 0`, agents
answer only users.

A user's message starts a new chain. What an external agent says unasked when a long task ends
continues the context of its answer to the request that began the task, so it is as many hops away
as that answer was.

The channel's own limit on routing steps is a different thing: a safety net against faulty routing,
not a rule for users.

The REST API returns each channel's `moderator` and `responder_policy`, and the web UI explains the
policy to the user (see [WEB-UI.md](WEB-UI.md)).

### `@`-Mentions

A user can address participants directly with `@handle` (e.g. `@sassy` for "Sassy"). A handle is the
participant's name transliterated to ASCII and lowercased, other characters collapsed into `-`
(`caelumora_model::mention`): "Zoë" is `@zoe`, and `@Zoë` addresses her too. Unknown handles stay
plain text, and text in backticks is ignored.

The channel resolves mentions once, as a message enters it, and stores the resolved ids with the
message. Nothing downstream parses the text again, so a later rename cannot change whom a past
message addressed.

A mention changes who is asked to respond, not who receives the message: every participant still
gets it, but every moderator asks exactly the mentioned agents instead of applying its own policy.
Mentioned agents that are asleep are woken first (see [Waking](#waking)).

Mentions written by agents are routed as well: an agent can hand a question to another
(`@researcher, can you look this up?`). The limit on chains between agents keeps two agents from
addressing each other indefinitely; an agent that mentions itself or an author of the message is not
asked.
