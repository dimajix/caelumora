# Events

Changes in Caelumora are announced on an event bus: the creation, change or deletion of a session,
channel, agent or user. This document describes the form of an event and the `EventBus` in the
runtime: how it is subscribed to and how a subscriber receives only what it may read. The
server-sent event streams through which clients follow these events are part of
[REST-SERVER.md](REST-SERVER.md); what happens inside a channel is in [CHANNELS.md](CHANNELS.md).

## The form of an event

An event about an entity named by its own id - a session, an agent, a user, a user's profile, a
workspace, a skill, an artifact, a group, or a kind of a plugin - is one variant,
`GlobalEvent::Entity { kind, id, change }` (issue 323). `kind` is the `EntityKind`, `id` the
entity's id as text, and `change` an `EntityChange`:

*   `Created`, `Deleted`, `Restored` - the same for every kind;
*   `Updated(value)` - what changed, in the form of the kind's update event (`SkillUpdateEvent`,
    `ArtifactUpdateEvent`, …); a profile comes as it now stands;
*   `Custom { name, data }` - something particular to the kind, under a name its module defines:
    `session::STARTED` and `STOPPED`, `agent::STATE_CHANGED`.

An event is made by the kind it is about - `GlobalEvent::created::<Skill>(&id)`,
`updated::<Session>(&id, &update)`, `custom::<Agent>(&id, agent::STATE_CHANGED, &())` - and read the
same way: `event.about::<Agent>()` answers the agent's id and the change, or `None` for an event
about anything else, and `change.update::<AgentUpdateEvent>()` what an update says. The events of
a channel and of its members keep a variant each (`ChannelUpdated`, `AgentChannelJoined`, …), since
they name the session as well, which the web UI needs to find the channel.

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
these subscriptions are served directly with everything on their topics.

Each subscription is a broadcast channel of its own, of bounded capacity. A subscriber that falls
too far behind loses the oldest events, and it is the subscriber that learns so (`Lagged`), not only
the log; one that cannot catch up logs the loss and goes on. One that can takes the receiver itself:
a running session that lost events looks at all its agents again (see
[CHANNELS.md](CHANNELS.md)).

### Publishing with attributes

`publish(event, attributes)` takes, next to the `GlobalEvent`, the `SecurityAttributes` of the
entity it is about: the session's, the channel's, the agent's, or, for user events, attributes owned
by that user. The bus keeps both together internally; subscribers only ever receive the
`GlobalEvent`, and the attributes never leave the process.

What is classified - a channel and what happens in it, a workspace, a skill, an artifact - is
published with `publish_classified(event, attributes, classifications)`: its classification as
well, or, as it is reclassified, the one it had and the one it has (issue 312).

### Subscribing as someone

`subscribe_as(context)` receives the events about the entities `context` may read. It asks the
`SecurityManager` the same question the `EntityStore` asks for reading them. The events of a
channel - its creation, change, start and stop, who joins and leaves it, an agent's state in it - go
by the channel's attributes and are asked as reading the channel. A user's record is everybody's to
read, their profile only theirs, so an update of a `user_profile`, which carries the profile,
reaches nobody else. A deletion is decided by the attributes it was published with, since the entity
is gone by then.

An event published with classifications reaches only who is also cleared for one of them, as the
views reach a classified entity: a reader of a channel they are not cleared for does not hear of
it, as they do not find it. A reclassification reaches who was cleared for it before or is now, so
that whoever no longer reaches it learns so.

These subscriptions are served by one dispatcher task. The policy is asked asynchronously, and
`publish` must not wait for it; a single task keeps every party's events in the order they were
published.

## Who follows the bus

The REST server's global stream (`GET /events`) is a `subscribe_as` of the logged-in user, and the
web UI's session list follows it. How the stream starts with a snapshot, resynchronises and ends
with its token is described in [REST-SERVER.md](REST-SERVER.md#streams). A channel's stream carries
the channel's envelopes, not global events; it is described there too.
