# 39 — The global event stream is not filtered by what a user may see

**Status:** open
**Area:** Identity & access management

## The problem

Since [11](11-agent-ownership.md) the global stream's snapshot lists only
the sessions a user may read, and `GET /sessions/{id}` answers `404` for the
others. The events that follow are still sent to everybody:
`SessionCreated`, `ChannelCreated` and the rest name sessions a user may not
see, and `SessionUpdated(NameChanged)` carries the new name of a session the
user has no access to.

The web UI does not show them - fetching such a session answers `404`, which
it ignores, and a rename only applies to a session already in its list - but
the ids and names go over the wire to anyone logged in.

## Why it matters

A session's name is chosen by its owner and can say what it is about. Ids
alone are less telling, but they let a user learn how many sessions exist
and when others are active. Both are exactly what the ownership policy is
meant to keep to the people involved.

## Considerations

- **Filter in the stream, per user.** For each event, ask the
  `SecurityManager` whether the user may read the session it names, with
  the session's attributes. That needs the attributes at hand: a running
  session has them in memory, a stopped one in the store - a store read per
  event and subscriber would be too much, so a small cache of attributes by
  session, updated from the same events, is likely needed.
- **Events about a session one loses access to** (being removed as a
  contributor, once that is possible) should end with something that tells
  the client to drop it, e.g. a `SessionDeleted` from that user's point of
  view, or simply a fresh snapshot.
- **Events that name no session** (`UserCreated` and the like) need a rule
  of their own; today nothing in the web UI uses them.

## Done when

- A user's global stream carries only events about sessions they may read.
- The web UI's session list still follows everything it shows.

## History

- **2026-09-24** — Opened while implementing [11](11-agent-ownership.md),
  which filtered the snapshot but not the events.
- **2026-09-24** — Discussed, not decided; to be picked up again. What was
  worked out:
  - **Where to filter:** in what goes out to a user, not in the `EventBus`
    as a whole - the runtime's own subscribers (`SessionLoop`,
    `ChannelLoop`) need every event.
  - **The core difficulty:** deciding needs the attributes of the session
    an event names, and for `SessionDeleted` the session is already gone
    from memory and store.
  - **A - a per-connection set of visible sessions** (recommended so far).
    Filled from the snapshot the store already filters (computed without
    sending it if the client asked for none, rebuilt on every resync after
    a loss, issue 38). On `SessionCreated` the stream asks once whether the
    user may read the new session and adds it; every other session event
    passes only for sessions in the set, `SessionDeleted` removes it
    afterwards. No global cache, no change to `GlobalEvent`, and the stream
    handles events in order, so nothing about a new session is lost between
    `SessionCreated` and it being added. Limit: the set stays right only
    while owner and contributors of an existing session do not change -
    true today (no participants can be added, issue 09); once they can,
    `UserChannelJoined`/`Left` must re-check the session. Drawback: other
    users' events still take room in the subscription's buffer (128) before
    the stream drops them, so heavy traffic elsewhere can make a stream lag.
  - **B - `GlobalEvent` carries the session's attributes**, so each event
    can be decided on its own, deletions included. Changes the event model
    for every subscriber, and the stream would have to strip the attributes
    again before sending.
  - **C - a global cache of every session's attributes**, kept current from
    the same events. More general, but a second state that must agree with
    the store, for one use.
  - **Topic routing in the bus.** The stream subscribes with
    `subscribe_all()`, i.e. `SubscriptionTopic::All`. Subscribing to the
    visible sessions' `Session(sid)` topics instead would let the bus
    filter, but a subscription's topics are fixed when it is made, and a new
    session's `SessionCreated` goes only to `All` and to its own topic,
    which nobody can have subscribed in advance.
    - `add_topic`/`remove_topic` on `EventSubscription` (keeping a clone of
      its own sender and a reference to the bus, finding its entry again
      with `broadcast::Sender::same_channel`) would not solve that: learning
      of a new session still needs `All`, and between noticing a session and
      adding its topic events can be missed.
    - Publishing session events additionally to `User(owner)` and
      `User(contributor)` topics would: a subscription to `User(uid)` - plus
      a `Shared` topic for sessions owned by the system - receives exactly
      what the user may see, new and deleted sessions included, without a
      race. It needs the session's attributes at `publish`, which the
      publishers (`SessionManager`, the controllers) have at hand; the
      `GlobalEvent` on the wire stays unchanged.
    - Found on the way: a subscription registered under several topics
      receives an event published to more than one of them several times
      (e.g. `ChannelCreated` goes to `Session(sid)` and `Channel(cid)`).
      That already applies to `subscribe(vec![...])` today; `publish` would
      have to send once per subscription across topics.
  - **`subscribe_filtered(topics, filter)`** (maintainer's proposal, a
    variant of A): the subscriber passes a filter the bus applies before
    sending, closing over the connection's current set of visible sessions.
    It matches an existing pattern - an actor's `event_filter()` applied by
    `FilteringEventSender` - and keeps other users' events out of the
    subscription's buffer. The concern raised: the filter runs at *publish*
    time, synchronously, with the set as it is then, while the set is
    updated only when the subscriber later reads `SessionCreated` and has
    checked (asynchronously) that the user may read the new session. The
    `ChannelCreated` published right after `SessionCreated` is filtered out
    before that, so events about a new session can be lost silently -
    harmless for today's sidebar, which refetches the whole session on
    `SessionCreated`, but the kind of silent loss issue 38 just removed.
    Letting through events of sessions not yet decided would close the gap
    but needs the filter to know the invisible sessions as well. Further
    constraints: the filter runs inside `publish` while the bus holds a
    `DashMap` entry - it must be fast, never block and never publish - and
    the set is shared between the publisher's thread and the stream
    (`Arc<RwLock<HashSet<SessionId>>>`). A possible middle way: A for the
    decision, plus a bus-side pre-filter later that only drops what is
    certainly invisible, if buffer pressure ever matters.
  - **Events that name no session** (`UserCreated`, `UserUpdated`,
    `UserDeleted`): the web UI does not use them and they reveal which users
    exist; proposed to not send them to users at all for now.

