# Events and Streams

Changes in Caelumora are announced on an event bus, and clients follow them
over server-sent event streams. This document describes the `EventBus` in the
runtime and the two streams of the REST server. What happens inside a channel
is in [CHANNELS.md](CHANNELS.md); logging in and renewing a session in
[SECURITY.md](SECURITY.md).

## The event bus

The `EventBus` (`caelumora_runtime::event`) carries `GlobalEvent`s: every
significant change in the system, such as the creation, update or deletion of
a session, channel, agent or user. It lets the REST server, the TUI and the
controllers react to changes without being coupled to whoever made them.

### Subscriptions

A subscriber names `SubscriptionTopic`s (`All`, `Session(id)`,
`Channel(id)`, `Agent(id)`, `User(id)`) and receives the matching events
through an `EventSubscription`. The runtime subscribes this way, and these
subscriptions are served directly with everything on their topics.

Each subscription is a broadcast channel of its own. A subscriber that falls
more than 128 events behind loses the oldest, and it is the subscriber that
learns so (`Lagged`), not only the log (issue 38).
`EventSubscription::recv` logs such a loss and goes on, for subscribers that
cannot catch up; one that can takes the raw receiver with `into_receiver`.

### Publishing with attributes

`publish(event, attributes)` takes, next to the `GlobalEvent`, the
`SecurityAttributes` of the entity it is about: the session's, the channel's,
the agent's, or, for user events, attributes owned by that user (issue 39).
The bus keeps both together internally; subscribers only ever receive the
`GlobalEvent`, and the attributes never leave the process.

### Subscribing as someone

`subscribe_as(context)` receives the events about the entities `context` may
read. It asks the `SecurityManager` the same question the `EntityStore` asks
for reading them (`GetSession`, `GetAgent`, `GetUser`). A deletion is decided
by the attributes it was published with, since the entity is gone by then.

These subscriptions are served by one dispatcher task. The policy is asked
asynchronously, and `publish` must not wait for it; a single task keeps every
party's events in the order they were published.

## The streams

The REST server exposes two server-sent event streams:

*   the **global stream**, `GET /events`, carrying `GlobalEvent`s;
*   a **channel's stream**, `GET /sessions/{id}/channels/{cid}/events`,
    carrying `Envelope`s.

Both are built by one `sse::stream`: snapshot, resync on lag through
`sse::resync_on_lag`, and the end at the token's expiry or when the watch
ends (issues 38, 104).

### Who follows a channel

A channel is followed live only by its participants, through their own actor
(`UserActor::watch`, issue 104). The stream is every event the channel
publishes, and it ends when the actor ends: when the user left the channel,
or when the channel was stopped or deleted. Deleting a channel stops its
actors, whether or not they ran.

Anyone else who may read the channel - its session's owner, say - reads its
history over REST but does not follow it; the web UI shows them the history
and no input. Watching without taking part is issue 124, an invisible
administrator issue 125.

### What the global stream shows

The global stream is filtered by what the user may read: its snapshot through
the store (issue 11), its events through `subscribe_as` (issue 39).

Global events name sessions and channels by id alone. A client follows them
with `GET /sessions/{id}`, which answers in the snapshot's own form
(`SessionSnapshot`, mapped from the same `SessionView`) without starting a
stopped session.

### Snapshots

Both streams accept `?snapshot=true` and then begin with a named `snapshot`
event describing the state they start from (issue 34):

*   for a channel, its members and the last 20 messages, with `has_more`;
    older ones come over `GET .../messages`;
*   for the global stream, every session with its channels.

The order matters: the server subscribes *first* and reads the state
afterwards, so nothing that happens while the snapshot is assembled is lost.
The price is that an event may arrive twice, once in the snapshot and once as
an event; clients drop duplicates by `MessageId` or entity id.

When a client falls behind and events are lost, a client that asked for
snapshots is sent a fresh one instead of a note that something is missing.
This holds on both streams.

### Streams and the token

A stream does not outlive the token that authenticated it (issue 33). The
authentication middleware sees a stream once, when it connects, and never
again. So the server ends the response itself, five seconds before the token
expires, announced by a named `expired` event. `EventSource` reconnects, the
new request is authenticated like any other, and the snapshot it asks for
restores whatever happened in the gap. A client that is no longer entitled
gets a `401` there instead.

Every stream also begins with an SSE `retry` field of three seconds: the delay
a client waits after an *unexpected* break. On `expired` the web UI
reconnects at once, so the planned handover does not interrupt the output.

Streams authenticate through `require_stream_auth`, which differs from
`require_auth` in one point: it never renews the session. A stream that
renewed would keep its own session alive for as long as anything holds it
open - a tab in the background, or a client that is not a browser at all -
which is precisely what the renewal rules are meant to bound. A stream
therefore consumes a session; what extends one is `POST /auth/refresh` (see
[SECURITY.md](SECURITY.md)).

## In the web UI

Both of the web UI's streams go through one `openEventStream`
(`src/stream.ts`), so the snapshot, the reconnect on `expired` and the
session check on a broken connection are handled in one place.

The session list follows the global stream (issue 35). It takes its state
from the stream's snapshot, removes a session on `SessionDeleted`, takes a new
name from `SessionUpdated`, and fetches the session again on any event that
changes its channels or their members. Starting and stopping are ignored.
