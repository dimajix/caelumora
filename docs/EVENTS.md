# Events

Changes in Caelumora are announced on an event bus: the creation, change or deletion of a session,
channel, agent or user. This document describes the form of an event and the `EventBus` in the
runtime: how it is subscribed to and how a subscriber receives only what it may read. The
server-sent event streams through which clients follow these events are part of
[REST-SERVER.md](REST-SERVER.md); what happens inside a channel is in [CHANNELS.md](CHANNELS.md).

## The form of an event

Every event is about an entity - a session, a channel, an agent, a user, a user's profile, a
workspace, a project, a skill, an artifact, a group, or a kind of a plugin - and has one form,
`GlobalEvent { kind, id, within, change }` (issues 323, 345). `kind` is the `EntityKind`, `id` the
entity's id as text, `within` the id of the entity it lies in, and `change` an `EntityChange`:

*   `Created`, `Deleted`, `Restored` - the same for every kind;
*   `Updated(value)` - what changed, in the form of the kind's update event (`SkillUpdateEvent`,
    `ArtifactUpdateEvent`, …); a profile comes as it now stands;
*   `Custom { name, data }` - something particular to the kind, under a name its module defines:
    `session::STARTED` and `STOPPED`, `agent::STATE_CHANGED`, a channel's `STARTED`, `STOPPED`,
    `JOINED` and `LEFT` - with the `ActorId` of who joined or left -, `AGENT_STATE_CHANGED`, with
    the agent's id, and `SAID`, with a message others hear as it was said (see
    [CHANNELS.md](CHANNELS.md#what-is-new-to-a-user)).

What happens to a channel's participants is a change of the channel: its readers are the ones who
hear of it, and it carries the participant in its data.

`within` is there for a kind that lies in an entity (issue 335): a channel lies in a session, and
its events name the session, by which the web UI finds the channel and the event bus picks whom an
event reaches. Each kind says which kind of entity it lies in, `Stored::Within`, or `NoEntity` for
none (see [STORAGE.md](STORAGE.md#the-entity-store)); `within` is absent for such a kind. Like `id`
it is text, since the core does not know the id types of a plugin's kinds; who reads it knows the
kind, and with it what `within` is the id of.

An event is made by the kind it is about, at its address - its id, or for a kind that lies in an
entity that entity's id and its own (`Address<T>`): `GlobalEvent::created::<Skill>(&id)`,
`updated::<Session>(&id, &update)`, `custom::<Agent>(&id, agent::STATE_CHANGED, &())`,
`custom::<Channel>(&(session_id, channel_id), channel::JOINED, &actor)`. It is read the same way:
`event.about::<Agent>()` answers the agent's id and the change, `event.about::<Channel>()` the
session's id and the channel's together with the change, or `None` for an event about another
kind; `change.update::<AgentUpdateEvent>()` answers what an update says, and
`change.data::<ActorId>(channel::JOINED)` what a custom change says. Who asks about any kind - the
event bus, which kind of entity a receiver must be allowed to read - takes the fields themselves.
What an update does to its entity is the entity's own rule: `Session::apply`, `Channel::apply`,
`Agent::apply` and `User::apply` make what it says into the entity's values (issues 248, 353),
where it is made and where a change made elsewhere is taken over.

Written as JSON, an event is an object of these fields, `within` absent for a kind that lies in
nothing, a change of its life written as its name alone (`"change": "Deleted"`), an update as
`{"Updated": …}`:

```json
{"kind": "channel", "id": "<channel id>", "within": "<session id>", "change": {"Custom": {"name": "joined", "data": {"User": "<user id>"}}}}
```

An event about something other than an entity, should one ever be needed, is a decision of its own
(issue 345).

An event is read back as it is written: it is what the instances of a deployment will tell each
other (issue 122). That is why an update is carried as a JSON value rather than as a type of the
kind: a reader takes it the same way whether it was published here or in another instance, and an
instance that does not know a plugin's kind passes its events on all the same. The name of a kind
and of a custom change are read back as the constants that define them (`intern`); one no code
here defines is one nothing waits for.

## The event bus

The `EventBus` (`caelumora_runtime::event`) carries `GlobalEvent`s: every significant change in the
system, such as the creation, update or deletion of a session, channel, agent or user. It lets the
REST server, the TUI and the controllers react to changes without being coupled to whoever made
them.

### Subscriptions

A subscriber names `SubscriptionTopic`s - everything, or a session, channel, agent or user - and
receives the matching events through an `EventSubscription`. The runtime subscribes this way, and
these subscriptions are served directly with everything on their topics. An event about a session,
an agent or a user goes to its topic; one about a channel - and so about its participants - to the
channel's and to the session's it lies in, which the bus reads from the event's address. Other
kinds have no topic of their own.

Each subscription is a broadcast channel of its own, of bounded capacity. A subscriber that falls
too far behind loses the oldest events, and it is the subscriber that learns so (`Lagged`), not only
the log. Every subscriber makes up for what it lost by reading anew what it keeps (issue 502) - a
loss between instances will be the rule, not the exception:

*   the `ControllerManager` reads every live agent, user, workspace and project controller again as
    stored (see [STORAGE.md](STORAGE.md#following-changes));
*   a running session follows every agent taking part, and looks at all of them again (see
    [CHANNELS.md](CHANNELS.md)); its own document it does not read again, since a session runs in
    one instance and its controller is the one writer of it there, which holds every change of it
    live already;
*   an approver denies the pending requests of every agent that no longer runs in its channel, and
    decides the others again (see [APPROVALS.md](APPROVALS.md));
*   the `CommandReaper` checks the commands, as at any event that may concern them;
*   a stream sends its client a fresh snapshot (see [REST-SERVER.md](REST-SERVER.md#streams)).

So only tests skip a loss: `EventSubscription::recv`, which logs it and goes on, is built for them
alone (`test-support`).

### Publishing with attributes

`publish(event, attributes)` takes, next to the `GlobalEvent`, the `SecurityAttributes` of the
entity it is about: the session's, the channel's, the agent's, or, for user events, attributes owned
by that user. The bus keeps both together internally; subscribers only ever receive the
`GlobalEvent`, and the attributes never leave the process.

What is classified - a channel and what happens in it, a workspace, a skill, an artifact - is
published with `publish_classified(event, attributes, classifications)`: its classification as
well, or, as it is reclassified, the one it had and the one it has (issue 312).

An event about an entity as it stands is published with `announce(event, &entity)` (`Announcing`,
on every `EventPublisher`, issue 351): it reads the attributes and, of a classified kind, the
classification from the entity, so that what an event reaches is the entity's to say, not its
caller's. A leave reaches whoever left besides (`announce_also_to`, issue 23). Assembled by hand
are only the events that reach differently, and say why: a reclassification, with both
classifications.

### Subscribing as someone

`subscribe_as(context)` receives the events about the entities `context` may read. It asks the
`AccessPolicy` the same question the `EntityStore` asks for reading them. The events of a channel -
its creation, change, start and stop, who joins and leaves it, an agent's state in it - go by the
channel's attributes and are asked as reading the channel. A user's record is everybody's to read,
their profile only theirs, so an update of a `user_profile`, which carries the profile, reaches
nobody else. A deletion is decided by the attributes it was published with, since the entity is gone
by then, and asked as `HearDeleted`: whoever read it until then hears of it, also by a group that
went with it - the groups of a session's participants are deleted before the session's deletion is
announced, and the policy is asked later still. What a deleted session or channel announces as it
stops (`session::STOPPED`, `channel::STOPPED`) is asked as reading it and reaches its owner alone,
since it is stopped only once its groups are gone (issue
[458](../issues/458-deleted-while-it-runs.md)); its participants learn of it by the deletion.

An event published with classifications reaches only who is also cleared for one of them, as the
views reach a classified entity: a reader of a channel they are not cleared for does not hear of
it, as they do not find it. A reclassification reaches who was cleared for it before or is now, so
that whoever no longer reaches it learns so.

These subscriptions are served by one dispatcher task. The policy is asked asynchronously, and
`publish` must not wait for it; a single task keeps every party's events in the order they were
published. Each event is asked of all parties at once (`may_reach_each`, by
`AccessPolicy::check_each`, issue [503](../issues/503-bus-asks-every-stream.md)): the ownership
policy reads a group the event's attributes name once for all of them, not once per open stream -
for an event about a part, once for the whole and once for the part.

## Who follows the bus

The REST server's global stream (`GET /events`) is a `subscribe_as` of the logged-in user, and the
web UI's session list follows it. How the stream starts with a snapshot, resynchronises and ends
with its token is described in [REST-SERVER.md](REST-SERVER.md#streams). A channel's stream carries
the channel's envelopes, not global events; it is described there too.
