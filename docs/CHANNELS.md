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
channels' scopes, its workspace if made for it - goes with it (`ScopedEntities`,
`caelumora-runtime/src/scoped.rs`): deleting the session deletes them, restoring it restores those
deleted with it - not one deleted by hand before, nor one whose name was taken meanwhile -, purging
it purges them. They are found in the runtime's name, but deleted and restored in the name of
whoever deletes or restores the session (issue 320): if something is left they may not delete, the
session is kept and the deletion fails with `ScopeNotEmpty` (`scoped`, raised by the session as
`SessionError::ScopeNotEmpty`), saying how much of which kind is left; what they may not restore
stays deleted, and the session comes back all the same. The session is marked before its scope is
deleted, so that nothing of it goes unless the session's deletion was granted; all of it carries the
session's mark - one for all of it - and comes back with it (issue 469). If something is left, the
mark is taken away again in the runtime's name, and what was deleted stays deleted - restored by
hand, not with the session. Purging is done in the runtime's name. Each kind that may lie in a scope
is a `ScopeMember` (issue 323), and `ScopedEntities` goes through them in the order they were
registered. What has a manager of its own - agents, workspaces, skills, artifacts - is a member by
it (`…_lying_in`), deleted, restored and purged by it and announced as it announces its own
deletions: an artifact to whoever reads it, as when it is deleted alone (issues 319, 316).
Personalities and session templates, which have none, are members by their store (`Unowned`), which
marks them and announces nothing.

The groups of the participants of a session and of its channels lie in its scope as well, but are
not among what lies there: they belong to the session or channel itself, and its participants reach
it by them (issue 348). They go and come back with its mark, in the runtime's name, whoever deletes
or restores it - all of them or none (`GroupManager::delete_participants`, `restore_participants`),
announced as a team. Deleting deletes them last, once nothing else of the scope is left, so that a
session kept keeps its members; if they cannot go, the mark is taken away again, and they stay.
Restoring restores them first, with the mark; if they cannot come back, the session or channel is
marked again as it was - with them, so that a restoring tried again finds all of it -, and the
restoring fails. So a session or channel that is there always has the group of its participants
there, too. Purging purges them with it (`scoped::purge_for_good`).

That holds because what lies in a session's or a channel's scope is used only within it
(`Scope::admits`): an agent of it takes part only in that session's channels, or in that channel,
and a personality of it makes agents only within it. What lies in a user's scope or everywhere is
used wherever the permissions allow: that scope only says where its name is found. A session
template lies only in a user's scope or everywhere; one of a session's or a channel's would make
sessions it can never lie in, and is not stored.

Every session has a **project** its results go to (`Session::project`, issue 467): one of its own,
made with it in its scope, or a shared one it lies in, which it goes with. The session keeps its own
project as it keeps the group of its participants (`SessionManager`): made with it, in the runtime's
name; deleted and restored with its mark, the project after the groups, all of them or none; and
purged with it. What lies in the project's scope goes with the session as what lies in the
session's does, and what is left of it keeps the session as well (`ScopeNotEmpty`). A session is a
user's, so that its project has an owner. How a session is put into a project, and how its own is
kept, is in [PROJECTS.md](PROJECTS.md).

Every session has a privacy policy: whom it lets read what was said in it from other sessions. It is
taken over from its template (private unless the template says otherwise), changed only by the
session's owner, and shown to everyone who takes part. What it decides is described in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#reading-another-channel).

A session may be held in a time zone, an IANA name (`TimeZone`): the times of messages its agents'
models are given are written in it (see [AGENT-TURN.md](AGENT-TURN.md#who-said-what)). A session
holds it as the request names it, else as its template does, else in UTC - never in the server's,
which is the deployment's and would make what a model is given depend on the machine
(`Session::effective_time_zone`). The web UI presets a new session's from the browser. Whoever may
change the session changes it. It belongs to the session, not to its channels: its people are in one
place, and one zone in a session keeps the times read from another of its channels comparable.

### Creating and deleting channels

Channels can be created in, deleted from and restored to a running session
(`SessionLoop::create_channel`, `delete_channel`, `restore_channel`). Each is stored first, and only
then is the running session changed and the change announced:

*   **Creating** stores the channel (`CreateChannel`) and adds it to the session, which asks
    `UpdateSession`, so that only the owner may. A channel's name is unique in its session: the
    store refuses one the session has, when a channel is created, renamed or restored.
*   **Deleting** takes the channel out of the session and marks it deleted (`DeleteChannel`); the
    channel and its state are kept. What lies in its scope is deleted with it, as with a session:
    if something is left the caller may not delete, the channel is put back and runs on.
*   **Restoring** reads the channel as deleted (`ReadDeleted`), as the session lists its deleted
    channels, asks to restore it (`Restore`) before its participants' clearance, as creating does
    (issue 312), lists the channel again - the store asks `UpdateSession` - and starts it from its
    stored state. What was deleted with it comes back, but for one whose name was taken meanwhile or
    that the caller may not restore.
*   **Purging** is not a channel's: a deleted channel is removed for good with its session.

A deleted channel has its actors leave before its task is aborted: every actor is told to `leave`
and given up to `STOP_GRACE` to end, in which an external agent cancels its open remote tasks and
every actor stores its state.

### Ending an actor

An actor's task ends in one of two ways (issue 377), each a method of `Actor` without a default, so
that an actor cannot forget one:

*   **`stop`** - the channel stops, because its session is stopped or the process shuts down. The
    actor keeps what it is in the middle of, to be resumed when the channel runs again: an agent
    stores its turn at the step it stands at, an external agent keeps its remote tasks.
*   **`leave`** - the actor leaves the channel for good: removed from it, or the channel or its
    session deleted. An agent gives its turn up and says so, an external agent cancels its remote
    tasks.

Both are synchronous and go past the events, like `cancel`: they take effect at once, not after
what is queued. Every actor ends its own task. The channel's run loop alone owns the actors' tasks,
so that they end with the channel's; whoever ended an actor waits on its task separately
(`ChannelLoop::stop_actors`, `leave_actors`). All of them use the same grace: an actor that has not
ended after `STOP_GRACE` is aborted. A tool that hangs does not keep an agent: both interrupt it,
the call recorded as having run, and the agent ends within the grace.

### Shutting down

As the process ends - SIGINT or SIGTERM to the server, a signal or `exit` to the TUI - the sessions
are shut down in order (`SessionManager::shutdown`, issue 297). From then on no session is started:
a request that would start one is refused (`SessionError::ShuttingDown`). Every running session is
stopped as `stop` stops it, all at once, so that each actor keeps what it is in the middle of. Of
the commands, only what does not outlive the process ends: each storage decides
(`TerminalProvider::shutdown`) - the `local` shell ends its commands, an Open Terminal instance
keeps them, which the model can follow after a restart and their bound ends. The whole takes at most
`SHUTDOWN_BOUND`, eight seconds, within the ten Docker leaves between SIGTERM and SIGKILL; what is
still under way then ends with the process.

### Starting again after a restart

What a shutdown or a crash interrupted goes on by itself when the server starts again (issue
[388](../issues/388-resume-interrupted-work-after-a-restart.md)). Once it is seeded, the server has
`SessionManager::resume_interrupted` start every session in which an agent's stored state has work
under way: a turn stored at a step it goes on from, or a remote task the remote agent works on. The
state store keeps an index of them (see [STORAGE.md](STORAGE.md#work-under-way)), so nothing else is
loaded. Nobody asked for these sessions, so the runtime opens them in its own name; what the agents
then do is done in their own names, as at every start. They are started one after another, beside
the first requests, and not bounded otherwise: what they resume is the work that ran when the
server stopped, no more (issue [436](../issues/436-work-resumed-after-a-restart-is-not-bounded.md)).

Not started is a session whose agents only wait for a person - an approval, a question, a remote
task that asked back -, since the person's answer starts it; nor one deleted. A message delivered to
an agent but not yet taken in leaves the agent idle, so it waits for the next act in its session.

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

Every channel is built by `Channel::build`, from a template or on request. It lets the channel's
members read it: an entry in its access list naming the group of its participants, which follows
whoever takes part at the time and is kept by the `ChannelController` as someone joins or leaves
(see [SECURITY.md](SECURITY.md#groups)).

A channel's approver is one of its **internal actors** (`Channel::internal_actors`, issue 420):
actors that belong to the channel itself, as its moderator does. They are no participants - not in
`participants`, not in their group, not shown or offered as participants - and are never added,
removed or announced; they have no range of the history, and what is recorded to one goes to it by
name. Otherwise an internal actor is an actor like any other: the channel builds it from its own
configuration, starts its task with the others as `run` begins, delivers to it by the one rule and
takes in what it sends. They are configured, like everything else of a channel: a template names
them for each channel (`ChannelTemplate::internal_actors`, an approver with its strategy, `Any`
unless named), and so does a request to create one (`ChannelSpec::internal_actors`; the web UI's
dialog names an approver). A channel has the internal actors it is given and no others - there is
no fallback: without an approver, nobody decides what needs approval, and tools that ask for it do
not work there.

The participants of a requested channel come from the session's participants, the creator always
among them. Its moderator and classification default as in a template (see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md) and [Moderators](#moderators)).

A channel is built and stored empty, and its founders join it by the one way in, as anyone added
later does (`ChannelLoop::found`, issue 420): each taken in, its actor started, and its joining
recorded in the channel's history - its `ParticipantJoined`, in the creator's name; nothing of the
history is shared with a founder, which is given what is said from its join on.
So every member of a channel has its join in the history, and the history alone says who was a
member when. Founding asks nothing beyond what anyone taken in is asked: naming the founders was
creating the channel (`CreateChannel`), so changing it is not asked, and it is reachable from
creating a channel alone - a requested one (`SessionLoop::create_channel`) and those of a template,
whose users and agents join the channels that name them once the session is made. Each founder's
clearance is asked before anything is stored, and again as they join; if one cannot join, the
channel is taken back. A channel's `run` applies the joins that wait as it begins, before it routes
anything, so that what a founder says comes after the joins of all who hear it. A channel started
again or restored records no joins: its members' are in its history.

A request that cannot be met is refused before anything is stored: no name, a name the session
already has, someone who is not a participant of the session, or a classification the policy
refuses - the last asked for channels from a template too. Components that make no moderator are
found only when the channel is built, after it was stored; it is then taken out of the session and
deleted again.

### Its language

A session and a channel may each be held in a language, a BCP 47 tag (`Language`). A channel is held
in its own, else in its session's - whatever that is at the moment, so that changing the session's
reaches every channel without one of its own (`Channel::effective_language`, issue 425). The rule is
the model's and answered there alone; the views, the prompt part and the REST API ask it. A channel
without a language of its own therefore cannot be held in none while its session has one: who wants
none leaves the session's empty too.

A session is held in the language the request names, else in the one its template names; a channel
in the one the request or its template names - one whose purpose comes with its language, such as a
German quiz -, else in none of its own. The server does not fall back to the language the creator
prefers: offering that is the frontend's, where the user sees what is chosen (issue 331). The web UI
and the TUI preset a new session's language from it where the template names none. Whoever may
change the session or the channel changes its language or takes it back. Its agents are told by a
prompt part (see [AGENT-TURN.md](AGENT-TURN.md#the-channels-language)); nothing said before is
translated.

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

`SessionLoop::run` runs every channel, those the session had and those created later - but for one
whose components cannot be read (see below).

Every actor of a channel is started the one way (`ChannelLoop::start_participant`): built, taken
in (`adopt`) and handed to `run` (`ActorChange::Start`), which starts its task - as the channel is
built, its participants and its internal actors, as someone is added, joins or founds it, and as
an agent is restored. They differ only in what they hand over: from which position the actor is
given the history, and on whose behalf it joined, if it did - a channel started again records no
joins. What waits when `run` begins is started before anything is routed.

*   Stopping the session stops the actors of every channel (`SessionLoop::stop_actors`), all at
    once, waits for them up to `STOP_GRACE` and then aborts every channel; the commands started in
    its workspace shell are ended. A stop therefore takes as long as its slowest actor needs to
    store its state. Deleting the session has the actors leave instead (`leave_actors`), as
    deleting a channel does.
*   Deleting a channel aborts that channel's task; its state is kept until it is purged. A channel
    deleted while it waits to be started is never started.
*   A participant that cannot be run is left out when the channel starts, with a warning, and the
    channel runs with everyone else: a user or agent deleted since it joined, or an agent whose
    personality or components were withdrawn or deleted. A third party - the owner of a personality
    or a component - must not keep someone's channel from starting. Such an agent is still a
    participant; adding it to another channel is refused.
*   A channel whose own components cannot be read - a bundle it names gone, or no longer its
    owner's to read - is left out as its session opens, with a warning, and the session opens with
    its other channels: by the same rule, one level up, a bundle's owner must not keep someone's
    session closed. The channel stays listed and is shown as unavailable, with why; it is read and
    deleted as any other, and whatever is to be done in it is refused with why
    (`SessionError::ChannelUnavailable`) - to whoever may read it; to anyone else it is not there.
    Its components are not changed after it was made: it runs again once they can be read, as
    the session is opened again - also while it runs, as every request in it does
    (`SessionLoop::retry_unavailable`, outside the lock that starts the session). A store
    that fails keeps the session from opening, as it does for a participant. Creating or restoring
    a channel, or a session from a template, is its owner's own request, and is refused instead
    (`SessionError::ChannelUnavailable`); a session from a template is rolled back.

### Who may see a channel

A channel its viewer may not read (`GetChannel`: owner and participants) is answered as absent and
left out of lists and snapshots - the same as one that does not exist. The runtime decides that, not
the REST server: `SessionLoop::channel_for` returns a channel only to whoever may read it, and the
views answer the same way. What a client lists and offers comes from views the runtime builds for
one viewer; see [SECURITY.md](SECURITY.md).

### What is new to a user

A user marks up to where they read a channel: the newest message of someone else they were shown,
never one of their own - their own is shown as they send it, before the channel takes it in, so it
may stand after a message of another that comes earlier in the history and was not shown yet. The
messages of others reach a user in the order of the history, since the channel appends and routes
in one step; so their positions in the history order them as the user sees them.

*   **Kept by the user's controller.** The mark is the user's state in the channel
    (`UserChannelState`, see [STORAGE.md](STORAGE.md#the-state-store)): the position of the message
    in the history and the message. The `UserController` is its one writer
    (`UserController::mark_read`), so that it is kept whether the channel runs or not, and for a
    reader who takes no part. It reaches the channel as the user reads it - the store asks - and
    takes only a message from where the user's view of the history begins. The mark only moves
    forward, so that two frontends of the user cannot move it back, and nobody else reads it.
*   **Counted by the views.** What is new (`ChannelView::unread`, `Detail::UNREAD`) is what others
    said that a participant hears (`Message::is_heard`: no steps of a plan, no notices of the
    runtime, nothing taken back), of what the user may read, after the mark and from where their
    view begins - counted up to a cap, and whether one of these messages mentions them, `@all`
    included.
*   **Announced as it is said.** Every message others hear is announced on the event bus once it is
    stored (`channel::SAID`), with the message, classified as the channel: it reaches who may read
    the channel and is cleared for it, which is who reads the message in the history. A frontend
    counts on from what it listed.

## Acting in a channel

Whoever writes or approves does so in their own name. `ChannelLoop::user_actor(UserId)` returns
*that user's* actor, and every envelope is built by the actor itself, so sender, name and
`SecurityContext` follow from it. There is deliberately no way to reach "some" user actor. Someone
who is not a participant of a channel has no actor in it and is refused.

A frontend talks to the user's actor, never to the channel. The actor queues what its user writes,
approves or announces, and its task sends it on through the transport, like every actor's output; no
actor takes a way around it. The queue closes when the actor's task ends, so that sending to a
stopped channel fails instead of waiting.

What the actor shows its user - what it is routed, and what its user says, as it forwards it - goes
to the user's outbox in the channel, which the `ActorManager` keeps across the actor's runs (issue
386). A frontend follows the channel through it (`ChannelView::user_follow`, a `ChannelWatch`),
also while the channel does not run, without starting it. Each run of the actor begins and ends in
it (`Shown::Started`, `Shown::Ended`): what a run showed comes after its start and before its end,
and an actor ended before it ran shows its end alone.

The channel does not take an actor's word for whose name it speaks in. Each actor is given an
`OriginEventSender`, which passes on only an envelope that speaks as the actor: as its sender, as
the caller of its `SecurityContext`, and as the sender of every message it carries. Anything else
is refused and logged as an error - a bug of the actor. Whose rights the actor has and who asked it
may be someone else: an agent acts with its owner's rights and answers whoever asked it (see
[SECURITY.md](SECURITY.md#in-whose-name-an-agent-acts)). What the channel says itself - a
moderator's notice, a removal - does not come through an actor's sender.

**Nothing said is unsaid.** The channel keeps every message it admits in its history as it arrives,
the steps of a plan included, records where each step of a plan begins, what an agent did of its own
accord and what came of it (`ActivityFinished`, issue 284), and after a turn's messages how it
ended. A turn given up keeps what it said; only a message still streaming when it ended is lost,
never having been complete. How a turn ended is told by whoever had it, also by an agent removed
while it answers: the channel takes what a removed actor sends until its task has ended, and records
no end in anyone's name. A turn whose agent was aborted after the grace, saying nothing, stays
without an end: nobody ended it.

**Everything shown is recorded** (issue 417). Every entry says who sent it and whom it went to:
every member at the time it concerns by the rule every channel delivers by - resolved as it is
recorded, so that who was a member then decides who reads it -, or those the moderator named. What a
participant was shown live is what the history holds for them, in the same order - also what went
to them alone: the answer to a request to answer again, a command's answer and a command not carried
out, a chain cut off, that it could not be decided who responds, a refusal, a request to respond,
each notice and request recorded in the system's name as the channel sends it. What it delivers
carries the position of its entry (`EventContext::position`), the `seq` of the item a client reads
it as later. Not recorded: what streams, a beginning whose end is recorded, where an actor stands,
approvals and questions.

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
| `RetryRequested` | `RetryMessage` of the channel; a message, not taken back, or a turn, of an agent taking part |
| `QuestionAnswered`, `QuestionDeclined` | `AnswerQuestion` of the channel - whether the sender was asked is the agent's to check |
| `CommandRequested`, `NudgeRequested` | sent by a user, naming an agent (or, for a nudge, anyone else) taking part |
| `ApprovalGranted`, `ApprovalDenied` from a user | `ApproveToolCall` of the agent it names - whether that agent asked is the approver's to check (issue 312) |
| `ApprovalGranted`, `ApprovalDenied`, `ApprovalTimedOut` otherwise | sent by an approver: its decision |
| `PlanStepStarted`, `CommandExecuted` of a turn, `ActivityStarted`, `ActivityFinished`, `TurnFinished`, `TurnGivenUp` | sent by an agent in its own name: nobody else has turns, and the history records them (issue 284) |

A message is admitted as it arrives, so one said after the channel was lowered below its turn is
refused. What is refused is written nowhere and delivered to nobody. The channel tells its sender
alone why, in the system's name (`MessageRejected`, with a `Rejection`), recorded to the sender
alone (issue 417). A
user's actor shows what its user sends before it forwards it, so that the refusal always follows
what it refuses (see [WEB-UI.md](WEB-UI.md)). What an agent said that the channel refused stays in
its own transcript only.

Waking the channel for a message is the actor's job, too: it sends a `WakeupRequested` ahead of
every message. Neither a frontend nor the channel decides that by itself.

User commands (`/help`, `/wakeup`, `/nudge`) are run by the user's actor in the same way; a command
an agent carries out itself is sent to that agent, which carries it out in a turn of its own. See
[COMMANDS.md](COMMANDS.md).

## What a message carries

A message says what it says in text, and may carry attachments besides - or alone (issue 282):
content a person or an agent attached, each classified on its own. An attachment is a reference to
content (`ContentRef`) in the message's `attachments`: its bytes lie in a storage of content (see
[STORAGE.md](STORAGE.md#content)), and the reference names them by the storage, their key there and
the digest of the bytes, which is what they are checked against, never how they are found. An
attachment is part of what was said: heard, published, modified and taken back with its message, and
covered by its digest. What a tool gave its model besides its text is a reference of the same kind,
but no attachment: it stays with the tool's result (`ToolCallResult::media`), seen rather than said,
and is withheld with it.

A person attaches a file in two steps. First they upload it for the channel
(`AttachmentManager::create`): as a participant of the channel, classified as they say - the
channel's classification unless they say otherwise - so that it may flow into the channel and into
the storage it is kept in. What it is is told by its first bytes, never by its name or by what
the client claims; any file may be uploaded. It is kept at once, under `attachments/<channel>/<id>`,
and waits for the message that names it, in memory, as long as `attachments.unclaimed_expire_secs`
says; one that waited longer is removed as the next upload is kept. Then their message names it by
its id: their actor puts the reference into the message (`AttachmentManager::named`), and the
channel admits it only as it was uploaded - by the sender, for this channel, not yet claimed, at
most `attachments.max_per_message`, none twice, each flowing into the channel - and claims it as it
lets the message in: no other message can carry it then. A modified message keeps what it carried
and may add what its author uploaded. What a removed attachment was stays in the storage, as what
was said stays in the history; every attachment of a session goes when the session is purged.

An agent attaches what it heard by its number (`attach`, see [TOOLS.md](TOOLS.md#attachments)): a
copy of it under an id and a key of its own waits for the agent's answer as an upload does, and the
channel claims it as it claims one. What a tool's result showed its model again (`shown`) the
channel admits where it lies in the channel's key space - one of its attachments - and its label
flows in; what a tool gave its model (`media`) it refuses until issue 208 brings the first.

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
it failed, the channel was lowered or it reached the limit of its model calls, while nothing
followed it. It is a request to the agent, not an operation of the channel: `RetryRequested`,
naming a message or a turn.

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
    *   a request of its last turn, given up because it failed, the channel was lowered or it
        reached the limit of its model calls, with nothing after it, is accepted and answered anew
        after it - nothing taken back; otherwise it is rejected;
    *   of the requests of a message, the last is judged against the conversation as it stands, the
        others rejected. It is accepted only if the message is the agent's last answer, as its own
        transcript says: then the agent takes back the message and the rest of its turn, in its
        own state and in the channel. Otherwise it says why, as one of a known few reasons
        (`RetryRefused`), which a user interface words itself.

    A retry accepted and a request to respond in the same batch are one turn, in the request's
    contexts; its model is given the conversation as it stands, without what was replaced.
*   **An external agent** rejects every request to answer again: its remote agent keeps what it was
    given, and nothing here takes the answer back from it.

Whether a retry was accepted is told to the users and recorded for them (`RetryAccepted`,
`RetryRejected`, issue 417). Whether a viewer may ask is part of each participant's snapshot - the
viewer's `RetryMessage`, of an agent whose kind answers again - so a client offers it only where it
is permitted and can succeed.

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

Who takes part in a channel and whether they may is one question, owned by `Participation` (issue
356), which the `RuntimeManager` holds once for the instance with the services it is asked with: the
agents' store, the controllers, the `AccessPolicy` and the `ConfidentialityPolicy`. It resolves a
participant as a document names it into a `ResolvedParticipant` - what its actor is built from, and
the place it takes part in - and answers whether it may run, join, lie where it may and is cleared
for the channel. The session and channel loops and the `SessionManager` ask it alike: when a
channel is created, restored or built, when someone is taken in or an agent comes back, and when a
channel is reclassified.

### Taking someone into the session

Who is not in the session yet is taken into it first, and with a channel named into that channel at
once - by whoever may change the session (`SessionLoop::add_participant`), or by their own act where
an entry of the session's access list grants them `Join` (`SessionLoop::join`;
`SessionManager::join` opens the session for one who may join but not read it). Who joins takes in
themselves or their own agent, never someone else.

*   **Everything is asked before anything is stored**: changing the session and the channel - or,
    for one who joins, the `Join` entries of the session and the channel, in their own context -,
    reading the one taken in - a user to know they exist, an agent so that nobody brings in an agent
    whose tools they may not use -, and whether they may take part in the channel: where an agent
    lies, and their clearance, an agent's by its owner and its personality. Only the channel is
    cleared for; joining the session as such asks no clearance, since each of its channels is read
    by those cleared for it.
*   **The session first, then the channel.** One the channel refuses after all stays in the session.
    Who is in the session already is added to a channel by whoever may change the channel alone.
*   **Stored in the runtime's name** for one who joined: joining was the act that was permitted. A
    personal `Join` entry - an invitation - is used up then, so that one removed later cannot take
    themselves back in; an entry for a role, a group or everyone stays. Who joins a channel by
    themselves sees all of its history: whoever let them join let them read it.
*   **What follows**: the session's document gains the participant, the group of its participants
    follows before it is announced (`SessionUpdateEvent::ParticipantJoined`), so that the new one
    hears of it, too, and a running session follows an agent taken in as it follows those it started
    with.

Whom one could take in is the session view's to answer (`SessionView::candidates`): the users and
agents whose names begin with what was typed, each with whether they take part already and why they
could not be taken in - coarsely, never which clearance is missing. Only whoever may change the
session searches: a search tells who exists.

A removed participant is kept as a former one, with who removed it and when, not among those who
take part now - so nothing that reads them changes. The views reach former participants, so what an
agent said and saw stays readable for whoever may read its context.

Deleting an agent (`AgentManager::delete`) marks it and changes no channel: it rests in every
channel it takes part in until it is restored. The channels follow its mark instead:

*   a running session, told of the deletion, has the agent's actor leave each of its channels, in
    the runtime's name, and announces it as a leave (`channel::LEFT`);
*   a channel that starts leaves it out, as any participant that no longer exists -
    `Participation::resolve` asks the store whether it exists, not the controllers, which keep an
    agent's controller while an actor of it still runs;
*   the views show it as a former participant, removed as its mark says, and keep its name, so that
    what it said keeps its sender.

A running session subscribes to the events of its agents, and looks at all of them again as it
starts and after it lost events, so that a deletion or a restore it did not hear of is not missed.

Restoring the agent is refused if another agent took its name in its scope meanwhile. Otherwise it
takes part again in every channel it rested in: a running session builds its actor anew, gives it
the part of the history it may see, as when it joined, and announces it as a join. Removing a
resting agent from a channel changes the document alone, and the agent does not come back there.
Purging it removes its document and its state, its own and in every channel; in the channels it
rested in it stays a former participant, without when it left. An agent of a session's or a
channel's scope goes with that session or channel (see [Sessions](#sessions)).

### How much history a newcomer sees

Whoever adds a participant decides how much of the history so far they may see: all (the default),
none, the last n messages or the last n days (`HistoryAccess`). It is fixed at once as the position
in the history the participant's view starts at, so "the last three days" does not move day by day;
to fix it, the history is read back from its end only as far as the grant reaches.

Before the newcomer's actor starts, the channel records what it was given - `HistoryShared`, with
the position and who granted it - right before its `ParticipantJoined`, in the system's name and to
the newcomer alone: what it reads of the history is a delivery too, and the history says what every
participant received (issue 417). It is recorded only if it shares something, if the position lies
before the join; a newcomer granted nothing is given what is said from its join on.
When a participant leaves, its `ParticipantLeft` says from which position on the channel delivered
nothing to it any more (`until`) - where it stopped, which may lie before the entry, since what the
leaving actor still sends comes first (issue 262).

So the history alone says who was a member when, and every read of it applies that, without the
channel's document (issues 379, 417): what went to every member is given to whom it was one then -
from the position shared with them, or else from their join, up to where delivery stopped (see
[STORAGE.md](STORAGE.md#histories)) - over REST, in the snapshot, for `read_history` (see
[MEMORY.md](MEMORY.md)) and for what a joining actor is given. A former participant reads what went
to it while it took part, and nothing after.

*   A **joining actor** reads what it was given through its inbox (issue 223, see
    [Actors and envelopes](#actors-and-envelopes)): the inbox opens behind, at where its view
    begins, and reads the history on from there as the actor - a participant whose membership the
    join just recorded -, then what is delivered from there on. An agent takes it in as what it
    hears, and keeps what passes its perceive processors in its transcript; an actor that keeps
    nothing - a user, the approver - opens at the end, and reads its own join alone. Nothing the
    history says it was given escapes it, and nothing comes twice (issue 423): there is no catching
    up beside the inbox.
*   A **rejoining actor** opens where it stood, as its state stores it (`AgentChannelState::heard`),
    or where its new view begins if that lies after. The transcript is only ever appended to, so
    older messages that a wider grant makes visible would land after the agent's own later answers;
    the agent reaches them through `read_history`, and what it keeps stays in the order things were
    said.

Notes stay visible to everyone in the channel: they are written on paper, not part of the
conversation.

### What changes in the running channel

The channel starts the new actor's task, tells the moderator and rebuilds the mentions; a handle
that becomes ambiguous is logged.

A removed actor leaves at once - an agent without finishing its turn, which it gives up and says
so - and its state is kept, for the record and for a later return. The approver answers a leaving
agent's pending approvals as denied, so that nothing keeps waiting, and decides the others again,
since who may approve them changed. The approver itself is no participant and is never removed.

A participant added again while its old actor still ends waits for it, at most a little past the
grace, and is refused as still leaving after that. The new actor thus loads the state as the old one
left it, and never runs next to it. What the old actor sent before it ended is routed before the new
one starts under the same id. An actor removed before the run loop started it is never started.

### How others learn of it

A join or a leave is announced on the event bus like every change of membership
(`channel::JOINED`, `channel::LEFT`). In the channel itself, it is the channel's own word (issue
408): `ParticipantJoined` and `ParticipantLeft`, which no actor may send (`admission` refuses them),
told in the name of whoever made the change - who added or removed the participant, the participant
itself if it joined by itself, `System` if the runtime did, as for an agent deleted or restored. The
event is named for what happened to the participant; who made it happen is its sender. Each
carries the participant's name as it was, so that it reads the same after a renaming.

*   **Recorded where it happened.** The channel keeps it in its history
    (`HistoryEntry::ParticipantJoined`, `ParticipantLeft`, with who made it and when), among what
    was said, as it keeps the end of a turn.
*   **A join** is announced before the newcomer's actor starts, after where its view of the
    history begins was fixed - right after `HistoryShared`, which records what it was granted, to
    it alone, if anything -, so that it is recorded before the newcomer reads, which reads it after
    what it was granted. A channel founded with its participants' joins recorded (issue 420) starts
    its agents as after a restart: one that kept nothing yet reads the founding joins from the
    history, as a founder joining live hears them.
*   **A leave** is announced once the actor's task has ended, and at the latest before an actor
    starts under the same id. Until then the channel takes what the actor still sends; after it,
    nothing of the actor enters the channel. That is how a frontend ends what an actor aborted
    without a word left under way.
*   **Told to everybody**: every moderator forwards it to the users and the agents, also to whoever
    it is told in the name of - nothing of theirs is echoed back to them; it is no word of theirs.
    An agent takes note of it without responding (see [AGENT-TURN.md](AGENT-TURN.md)).

A resting agent removed from a channel changes the document alone (above): nothing runs that could
leave, and nothing is announced in the channel.

A removed user's live stream of the channel ends because it is theirs: it is followed through their
actor, which ends when it is stopped, and the stream then finds them no longer a participant (issue
386), so no one has to listen for the announcement.

Every channel response carries `can_change`, so that a client offers adding and removing
participants only where the user may (see [WEB-UI.md](WEB-UI.md)).

## Acting on the channel as a whole

Cancelling and waking act on the channel as a whole. They ask the `AccessPolicy` first, one
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
*   **What remains of it** is the request to respond, recorded to the one asked (issue 417), and
    the answer: nobody but the one asked is shown the request, and the command answers the issuer
    alone.

### Refusals

A refusal is a `ChannelError::Forbidden`, which REST answers with `403`. Like a refused tool call,
it says what was refused, not which rule refused it.

## Actors and envelopes

An **actor** is anything that sends and receives events in a channel.

*   **`Actor`** (`caelumora_runtime::actor`) is the trait of all actors: `AgentActor`, `UserActor`,
    `ApproverActor` and `ExternalAgentActor`. Its task, `run`, receives envelopes and sends its
    output through the transport it is given. `wakeup`, `cancel`, `stop` and `leave` are
    synchronous, because they come from other tasks and must take effect at once. The trait is
    public, so that a kind of agent can be built in a crate of its own (issue 370); only the channel
    controls its actors, after the operation that does it checked its permission - a rule for linked
    code, not a boundary (see [SECURITY.md](SECURITY.md#operations-that-check-themselves)). What an
    agent's model saw is read through the views, not asked of its actor (see
    [AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw)).
*   **An actor's inbox** (`channel::inbox`, issue 223) is a bounded queue of the envelopes the
    channel delivers live, and behind it the channel's history, which it reads from wherever the
    queue fell short. The channel opens it as the actor starts, at where the actor stands in the
    history (`Heard`): its stored mark after a restart, where its view begins as it joins, the end
    for an actor that keeps nothing. Opened before the end, or once its queue is full, the inbox is
    *behind*: a recorded envelope is dropped - the history has it - and read from the history once
    the queue is empty, as the actor, from where it stands, until nothing more is to read; then the
    queue takes over again. An overflow, a restart and a join are one case. What is not recorded is
    transient and may be lost while the inbox is behind (`ActorEvent::is_transient`) - streamed
    chunks, where an actor stands, the start of what ends recorded -, except the answers to
    questions, which the history never holds (issue 165) and the inbox queues regardless. A live
    envelope carries the security context its sender set as it sent it; one read from the history
    carries the recorded initiator and the sender's context as it is now (`Actor::security_context`,
    see [PERMISSION-CHECKS.md](PERMISSION-CHECKS.md)) - less exact, and the fallback alone. The
    actor's half (`EventReceiver`) takes from one sequence, queue then history, nothing twice: it
    can wait for the next envelope of a kind, taking it ahead of the others, which stay where they
    are - as an agent waiting for approvals does -, and says where the actor stands (`heard`),
    which an agent stores with what it took in. Both kinds of agent take an envelope together with
    everything there behind it: every message heard is taken in, in order, and only the last
    request is answered, once (see [AGENT-TURN.md](AGENT-TURN.md#the-cognitive-cycle)). During a
    turn, an agent whose personality hears then takes what it may hear only up to the first
    envelope that has to wait, which stays where it is with everything behind it (see
    [AGENT-TURN.md](AGENT-TURN.md#hearing-during-a-turn)).
*   **Which actor runs an agent** is its personality's `kind`: `internal` - the default - an
    `AgentActor`; `external` an `ExternalAgentActor`, for an agent that runs elsewhere and is
    reached through the personality's one `ExternalAgent` component. What builds the actor of each
    kind, and which facets its components are asked for, is registered with the runtime
    (`AgentKinds`, `RuntimeBuilder::agent_kinds`, issue 370) - both by the agents' plugin
    (`AgentsPlugin`), an essential one. The channel
    builds an agent's actor through them and names no actor of an agent itself, reading the
    personality as the agent may read it; a kind the program registered nothing for is refused
    (`ChannelError::UnknownAgentKind`). The agent is `ActorId::Agent` either way, and nothing else
    - the participants, the REST API, the moderator - tells the two apart.
*   **What both kinds of agent are in a channel** is their `AgentBody` (issue 355): the agent and in
    whose name it acts there, its `AgentChannelState`, how it takes in what it hears, its context
    provider, the commands it carries out itself, and its `RunControl` - where its actor stands and
    what `cancel`, `stop` and `leave` asked of it (issue 363). The two actors differ in their mind:
    a model run here with its tools, or the remote agent, whose every answer is a turn of the run
    control, ended by `cancel`, `stop` and `leave` as an agent's turn is. Both tell where they stand
    the same way; an external agent tells the begin and end of an answer at once.
*   **`ActorId`** identifies an actor: a `User`, an `Agent` or an `Approver`, or `System` - the
    runtime itself, set only deliberately.
*   **`Envelope`** is the unit of communication: a timestamp, a `SecurityContext`, an `EventContext`
    and the `ActorEvent` itself. The `SecurityContext` is what it is checked by, as its sender's
    actor set it, at the time it was sent; the `EventContext` where it happens and in which chain -
    its span, its depth, its hops between agents and the chain's initiator (issue 432), which what
    reads the chain goes by. A client is never shown the `SecurityContext` (`ShownEnvelope`, see
    [REST-SERVER.md](REST-SERVER.md)). Every envelope the runtime sends is built through `Answering`
    (issue 352): the contexts of a request continued in the actor's name (`to`) - an agent's, an
    external agent's, the approver's -, an actor's own (`own`) - a user's -, or, for what the
    channel says itself, its place in someone's name (`in_channel`): a removal in the name of who
    removed, a refusal in its own. Whose name an envelope carries is what the `OriginEventSender`
    and the admission check, so it is decided in one place. Every envelope is an event of its own,
    with an `event_id` of its own, even where many are sent in the same contexts - the events of a
    turn share its span and what it answers (`correlation_id`), not their id.

### External agents

An external agent has no turn and no tools here. It hears what others say and takes it in as an
agent does: what it hears through its perceive processors into its transcript. When it is asked to
respond - once for all it took in together, in the contexts of the last request - it gives its
remote agent:

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

A `ChannelLoop` is built with a `ChannelRuntime` alone, not with the `RuntimeManager` itself nor
with its `Channel`. Its session makes it (`SessionLoop::channel_runtime`), and it carries what
everything in the channel shares:

*   the process-wide services the channel is built with, each by an accessor of its own and read
    once by `ChannelLoop::new`, and never the `RuntimeManager` itself, so that what a channel can
    reach is what its runtime offers (issue 360);
*   the user commands of the session, built once per session;
*   the session: its controller, which guards the agents' tool calls with the session's rules, and
    its loop, by which the channel reaches its session and the other channels in it (issue 361) -
    a `Weak`, since the session owns its channels, and for the channel alone: the actors are not
    handed it (issue 123);
*   the channel's controller, made by the session with the runtime (issue 374): what
    `ChannelLoop::new` builds the channel from and holds, and what its actors read the channel
    by, so that the loop and its actors cannot hold two different channels;
*   a handle to the channel's loop - a `Weak`: the channel owns its actors, and a strong reference
    back would keep a stopped channel alive.

A session exists before its channels: `SessionLoop::new` makes it, and then builds each channel as
one created later is built (`add_channel_loop`), so that a channel has its session from the start.

The `ChannelRuntime` is the only source of `ActorRuntime`s, so every actor belongs to exactly one
channel. An `ActorRuntime` is made for one actor and hands out only what concerns that actor: the
services every actor needs - the model is not among them, an agent has it as a component - and those
its components may use; its own agent state and its state in the channel; its own event
subscription, for its channel and nothing else; the attributes of an agent that asks something of
it, as stored; what an actor asks of its channel - its classification and attributes, its users for
the questions of a tool call, the workspace as the channel may use it (issue 370); the controllers
of its session and its channel, which the actor reads them by and which guard its agent's tool
calls, held so that they do not lapse while a turn ends (issue 374); and its own session as the root
of what its work reads (`session_view`), from which the session's other channels are read as the
confidentiality rules allow (see [MEMORY.md](MEMORY.md#reading-other-channels)). It does not hand
out the `RuntimeManager` it is made from: what an actor may reach is read from `ActorRuntime` alone
(issue 359). It is the actor's interface to its channel, not a way to its `ChannelLoop`: the loop's
public API is the frontends', and acts in a user's name too. The loop is reached through it only by
the user's and the approver's actors and by the views, all in the runtime (`pub(crate)`, see
[CODING-GUIDELINES.md](CODING-GUIDELINES.md#runtimes-and-their-entities)).

The full `ChannelLoop` and `SessionLoop` are `pub(crate)`, so components from other crates cannot
reach them. Components are not given their actor's `ActorRuntime` either, only what their
`ComponentContext` holds; see [COMPONENTS.md](COMPONENTS.md).

## Moderators

A moderator decides whom an event goes to, who is asked to respond and what it tells of its own -
and never what anyone is told of an event (issue 418). It is a component of the channel: a channel
has components, from its template or the request, which the channel builds as an agent builds its
own - in the name of the channel's owner. `Moderator` and `ResponderPolicy` are facets of which a
channel has exactly one, each with a default (see
[COMPONENTS.md](COMPONENTS.md#facets-of-which-there-is-exactly-one)):

```yaml
channels:
- name: main
  components:
  - id: responders
    kind: mentions_only_policy
```

Every channel delivers by one rule, the channel's and not a moderator's (`Envelope::recipients`),
which for what the history records is the rule its entry is read by (`HistoryEntry::recipients`):

*   every message said goes, as it was said, to every other participant: to the users always, to the
    agents if there is something to hear in it - each agent takes in its words itself;
*   a change of a message, and who joined or left, go to every user and agent;
*   how an answer comes about - its start, partial and intermediate messages, plan steps - where an
    agent stands, how a turn ended and what an agent does of its own accord go to the users, not to
    the agents;
*   approvals, questions and commands go to whom they concern: a request for a command to the agent
    it names, that it began to the issuer, its answer to the issuer alone if private, else to every
    user; an approval through the approvers; questions to the users asked;
*   nothing goes back to its sender, but what the channel announces in their name.

A moderator answers each event with a `Routing`: whether it goes where the rule sends it or to some
participants alone, whom to ask to respond, and which of its own notices to tell whom - of a fixed
few (`Notice`): a chain cut off, and that it could not be decided who responds. The channel builds
every envelope from it: the event as it entered, a request to respond in the event's contexts, a
notice in the system's name. So a moderator cannot change what anyone is told, nor speak in
anybody's name.

The built-in moderator, `policy_moderator` (the default), lets every event go where the rule sends
it, and decides:

*   whom to ask to respond, when an answer is complete: for a message said outside of any turn as it
    is said, for the messages of a turn at its end. The decision follows from the messages - whether
    a turn is theirs - never from the kind of their sender;
*   every agent a message `@`-mentions is asked to respond, and nobody else: a message that mentions
    anyone is addressed to them, so one to people alone asks no agent (`mentions_replace_policy:
    anyone`, the default, issue 437). With `agents`, only a mention of an agent replaces the policy,
    and a message to people alone is answered by whom the policy chooses. A mention of a sender of
    the message counts for nothing;
*   a chain between agents ends at a limit (see below).

Whom it asks when a message mentions nobody whose mention replaces it is the channel's
`ResponderPolicy`, which is told the
event to answer, the conversation and which agents could answer, as an `ActorDescription` each:
what of an actor is public to everyone in its channel, its id, name and introduction
(`Actor::describe`), and nothing a policy from another crate could act on. The conversation ends
with what the event said - the message, or the messages of a turn -; before it come the last
messages heard, as many as the moderator's `context_messages` (default 10), as they stand after
every change, each naming its sender (issue 414). The moderator keeps them itself, from what it
routes: a message of a turn once the turn ended, so that a long turn does not push out what was said
before it, and nothing of a turn given up. After a restart it starts empty. A new way of choosing
who answers is a new policy:

| Policy | Who responds to a message without a mention |
|---|---|
| `broadcast_policy` | every agent |
| `round_robin_policy` (default) | the agents take turns, in the order of their ids; a mention does not take a turn |
| `random_policy` | one agent, chosen at random |
| `mentions_only_policy` | nobody: agents respond only when someone mentions them |
| `user_driven_policy` | every agent if a user wrote, nobody if an agent did |
| `decision_policy` | whom a decision model chooses by what was said (see below) |

Under the first three, an agent's answer makes other agents answer too: the agents can discuss with
each other. Under `user_driven_policy` the agents answer when a person writes and are quiet in
between: an agent's message is still delivered to the other agents, but starts no turn. Loops come
from asking to respond, not from delivering.

A policy that could not choose says so (`Undecided`, issue 441) - of those above, only
`decision_policy` can -, and the moderator asks whom its `on_undecided` says: nobody (`none`, the
default), so that a policy that is slow or failing leaves the agents quiet rather than makes every
one of them answer, or every agent the policy could have chosen (`all`), so that nothing said is
left unanswered. When nobody is asked, it tells the users so (`ResponderUndecided`, in the system's
name), recorded to them, after a user's message and an agent's alike: the channel is silent because
nobody could be chosen, and a mention asks someone. Why the policy could not choose is logged, and
told to nobody.

A `Routing` also says why it asks (`RequestReason`, issue 439): `mentioned` - by the agent's handle
or `@all` -, `nudged` by a user (see [Nudging someone](#nudging-someone)), `chosen` by the policy,
or `undecided`, asked under `on_undecided: all` because the policy could not choose (issue 441). The
request to respond carries it, and the history records it with the request. An agent whose
personality has the `request_reason_prompt_provider` is told, when only chosen, that it may stay
silent (see [AGENT-TURN.md](AGENT-TURN.md#why-the-agent-was-asked)): what a policy chooses is a
guess whether the agent has anything to add. Asked as undecided, it is told nothing, so that what
was said is answered as `all` intends.

### Choosing by what was said

`decision_policy`, of the plugin `caelumora-decision` (issue 403), asks a decision model which of
the agents should respond: a model that answers typed questions about a text with probabilities,
served by Ollama since 0.35 at `/v1/systemone` (see [LLM-MODELS.md](LLM-MODELS.md#decision-models)).
It is given the conversation and the candidates as they are introduced. Each message goes to the
model as a model is given one of another participant (see
[AGENT-TURN.md](AGENT-TURN.md#who-said-what)), its sender marked as `sender_format` and
`sender_style` say - by default `@handle: ...`, the name its options have, without when it was
written, which whom to ask does not depend on; what was said before is
left out beyond about 4,096 tokens, the oldest first. Up to 25 candidates it asks one
`choice` over their handles and "nobody": the most probable is asked to respond - nobody, if that is
most probable - and every other at or above `threshold` besides, at most `max_responders`. Beyond 25
it asks whether each should respond, up to 64, and asks those at or above `threshold`.

The channel waits for the decision before it routes anything else, so it is bounded by the
endpoint's timeout, 5 s unless written. What keeps the model from choosing - a timeout, an error,
an answer naming no option, more than 64 candidates - leaves the policy undecided, without anything
that was said, and the moderator asks whom its `on_undecided` says (see above).

What was said leaves the system where the endpoint lies outside it. The policy sends a message only
if it may flow to the endpoint's `destination` - `public` unless configured, as for an MCP server -
asked in the context of the event routed, in which the endpoint's credential is resolved too. What
was said before is left out if it may not; if what is to be answered may not, it is undecided as
on failure. Since its parameters say where what it is given goes, an operator configures it alone
(`ComponentKind::operator_only`): a channel created on request names it only through a `ref` to a
component bundle, while a template may write it.

| Parameter | Meaning | Default |
|---|---|---|
| `endpoint` | The server, its credential and `timeout_secs` (see [COMPONENTS.md](COMPONENTS.md#remote-endpoints)) | `url: http://127.0.0.1:11434`, 5 s |
| `model` | The decision model | `nimble` |
| `keep_alive` | How long the model stays loaded after a decision | the server's |
| `threshold` | How probable it must be that a candidate fits | `0.3` |
| `max_responders` | How many are asked at most | `1` |
| `destination` | What the server is cleared for | `public` |
| `sender_format` | How the model is told who said what: `none`, `prefix` or `xml` | `prefix` |
| `sender_style` | By which name a sender is marked: `handle` or `name` | `handle` |
| `prompt_texts` | Which variant of its questions the model is asked (see [CONFIGURATION.md](CONFIGURATION.md#the-texts-given-to-models)) | the default |

What the channel delivers is what its rule and the moderator's `Routing` say, and nothing else; it
has no broadcast of its own. Nothing goes back to the actor that sent an event but why it was
refused: an agent takes its own messages into its transcript itself, and a user's actor shows what
its user says itself.

### Chains between agents

An agent's message that makes other agents respond - because it mentions them, or because the policy
chooses them - is a *hop* of a chain between agents. The hops since the last message of a user are
counted with the envelope (`EventContext::agent_hops`); an agent's answer carries the count of the
request it answers. Once a request would exceed the moderator's `max_agent_hops` (default 3), it
asks nobody - nor its policy, so a rotation is left as it is. The message is still delivered, and
the moderator says that the chain was cut off (`ChainCutOff`, in the system's name), to the users
and to the agents involved - the sender and those it mentioned -, recorded to them (issue 417).
Since it is sent in another name, the channel delivers it to the sender too. With
`max_agent_hops: 0`, agents answer only users.

A user's message starts a new chain. What an external agent says unasked when a long task ends
continues the context of its answer to the request that began the task, so it is as many hops away
as that answer was.

Routing is bounded by the moderator alone; the channel asks it about every event (issue 422). Only
an ask makes anyone respond to what was delivered - a turn speaks on in the context of the request
it answers, an approval or a question is answered once -, so a moderator that bounds its asks
bounds every chain. Beneath the hops, the `policy_moderator` asks nobody to respond to an event as
deep in its span as `max_span_depth` (`EventContext::span_depth`, default 100): a net against
routing that goes on without end, not a rule for users. What was said is delivered all the same,
and nothing is told. A moderator of a plugin bounds its asks itself.

The REST API returns each channel's `moderator` and `responder_policy`, and the web UI explains the
policy to the user (see [WEB-UI.md](WEB-UI.md)).

### `@`-Mentions

A user can address participants directly with `@handle` (e.g. `@sassy` for "Sassy"). A handle is the
participant's name transliterated to ASCII and lowercased, other characters collapsed into `-`
(`caelumora_model::mention`): "Zoë" is `@zoe`, and `@Zoë` addresses her too. Unknown handles stay
plain text, and text in backticks is ignored.

`@all` and its synonym `@channel` address everyone at once: every user and agent taking part, those
whose name yields no handle included. They are group handles (`GROUP_HANDLES`), not names: a
participant whose handle is `all` or `channel` is among those they address, but cannot be
mentioned alone by it, and the channel warns of the handle as of one shared by several.

The channel resolves mentions once, as a message enters it, and stores the resolved ids with the
message - for `@all`, the participants of that moment. Nothing downstream parses the text again, so
a later rename, or a participant who joins later, cannot change whom a past message addressed.

A mention changes who is asked to respond, not who receives the message: every participant still
gets it, but every moderator asks exactly the mentioned agents instead of applying its own policy -
nobody, for a message that mentions only people, unless the `policy_moderator` is configured with
`mentions_replace_policy: agents` (see [Moderators](#moderators)).
Mentioned agents that are asleep are woken first (see [Waking](#waking)).

Mentions written by agents are routed as well: an agent can hand a question to another
(`@researcher, can you look this up?`). The limit on chains between agents keeps two agents from
addressing each other indefinitely; an agent that mentions itself or an author of the message is not
asked. Since every mention of an agent asks it, agents are told when to write a handle and when a
name - a handle only for an answer or an action needed from that participant now, never in a
greeting, thanks or praise -, in the roster and again before every turn's input (issue 442, see
[AGENT-TURN.md](AGENT-TURN.md#when-to-mention)): models take a handle for a form of address, and
agents that greet each other with `@` keep asking the next one to answer.
An `@all` asks every agent but the author, whoever wrote it, and needs no permission of its own: it
asks nothing that mentioning each participant would not. What it fans out to is bounded by the same
limit, as under `broadcast_policy`. An agent's question to `@all` (`ask_user`) is put to every user
of the channel.
