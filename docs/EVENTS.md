# Events

Changes in Caelumora are announced on an event bus: the creation, change or deletion of a session,
channel, agent or user. This document describes the `EventBus` in the runtime - how it is subscribed
to and how a subscriber receives only what it may read. The server-sent event
streams through which clients follow these events are part of [REST-SERVER.md](REST-SERVER.md); what
happens inside a channel is in [CHANNELS.md](CHANNELS.md).

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
the log; one that cannot catch up logs the loss and goes on.

### Publishing with attributes

`publish(event, attributes)` takes, next to the `GlobalEvent`, the `SecurityAttributes` of the
entity it is about: the session's, the channel's, the agent's, or, for user events, attributes owned
by that user. The bus keeps both together internally; subscribers only ever receive the
`GlobalEvent`, and the attributes never leave the process.

### Subscribing as someone

`subscribe_as(context)` receives the events about the entities `context` may read. It asks the
`SecurityManager` the same question the `EntityStore` asks for reading them. The events of a
channel - its creation, change, start and stop, who joins and leaves it, an agent's state in it - go
by the channel's attributes and are asked as reading the channel. A user's record is everybody's to
read, their profile only theirs, so `UserProfileUpdated`, which carries the profile, reaches nobody
else. A deletion is decided by the attributes it was published with, since the entity is gone by
then.

These subscriptions are served by one dispatcher task. The policy is asked asynchronously, and
`publish` must not wait for it; a single task keeps every party's events in the order they were
published.

## Who follows the bus

The REST server's global stream (`GET /events`) is a `subscribe_as` of the logged-in user, and the
web UI's session list follows it. How the stream starts with a snapshot, resynchronises and ends
with its token is described in [REST-SERVER.md](REST-SERVER.md#streams). A channel's stream carries
the channel's envelopes, not global events; it is described there too.
