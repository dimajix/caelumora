# Storage

Caelumora keeps two kinds of data apart: long-lived **entities** - definitions and identities - in
the `EntityStore`, and frequently changing runtime **state** in the `StateStore`. This document
describes both stores, who writes what, and the controllers that hold the entities of a running
session live. How access to stored entities is checked is in [SECURITY.md](SECURITY.md); which
stores are used is configured in [CONFIGURATION.md](CONFIGURATION.md).

## The entity store

The `EntityStore` trait (in `caelumora-model`) stores the long-lived domain entities:

*   **Personalities**: static definitions of agent traits.
*   **Templates**: `SessionTemplate`s, which sessions are made from.
*   **Identities**: `Agent`, `Session`, `Channel` and `User` records. A channel is a document of its
    own, which its session lists by id (issue 70).
*   **User profiles**: what a user says about themselves - so far their introduction (issue 176).
    One `UserProfile` per user, stored under the user's id; see [SECURITY.md](SECURITY.md#ownership)
    for why it is not part of the `User` record.
*   **Preconfigured components**: `ComponentDescriptor`s kept by id.

Every entity has a name, and the fields that mean something for it (issue 177):

*   **What is a scope** - a session, a channel, a user - has a name only.
*   **What lies in a scope** - a personality, a session template, an agent - has a name and a
    `Scope`: where it is found by name. A template finds its agents there, so an agent's scope
    decides whether it is reused - one per user, with its memory - or made anew for every session.
    Looking definitions up by name within their scope is
    [178](../issues/178-scoped-lookup-by-name.md).
*   What is offered for choosing - a personality, a session template - has a `description` as well,
    a catalogue line.

The type of a name says what it is for (`caelumora_model::name`):

*   **`Name`** - a name something is looked up by: a channel's. It is checked when it is made or
    read (not empty, no control characters, at most `MAX_NAME_CHARS`), kept as written with its
    whitespace tidied, and compared by its ASCII form (`any_ascii`) regardless of case - "Jonathan"
    and "jonathan" are one name, and so are "Zoë" and "Zoe", in a lookup, a map and a key. The form
    it is compared by is made once, with the name. Its `handle` is what it is `@`-mentioned by.
*   **`ScopedName`** - a `Name` with the `Scope` it lies in: what a definition is found by. Written
    into the document as the fields `name` and `scope`, the scope global unless named.
*   **`String`** - a name that is only shown (a session's), or that is decided elsewhere: a user's
    `username` is their login identity, owned by a user provider such as a directory, and opaque to
    Caelumora.

There are two implementations: `MemoryStore`, for tests, and `JsonFileStore`, which keeps one JSON
file per document.

### Demo data

Demo data (issue 105) is not part of any binary. The files that the configuration's `seed` names
hold personalities and session templates as the store keeps them, with fixed ids;
`examples/config.yaml` seeds `examples/seed.yaml`.

At startup, both binaries put into the store whatever it does not have yet, by id, as the system
(`caelumora_runtime::seed::load`). Restarting therefore changes nothing, and a document changed in
the store stays as it is. Without a `seed`, nothing is seeded.

## The state store

The `StateStore` trait (in `caelumora-model`) stores the runtime state of channels and agents:

*   **`ChannelState`**: the channel's history.
*   **`AgentChannelState`**: an agent in one channel - its processing state, history and transcript,
    its public and private notes, the texts of its prompt parts by digest, and the turn under way,
    if any (processing state, pending tool calls, plan, the turn's messages, changes not yet taken
    over). See [AGENT-TURN.md](AGENT-TURN.md).
*   **`ExternalAgentChannelState`**: an external agent in one channel (issue 179) - its history (all
    it was offered and all it answered), its transcript (what the remote agent was given - what
    passed its perceive processors, issue 196), up to which message of the history it offered
    anything on (`offered_up_to`), the remote conversation and the tasks given to it that have not
    ended, with what came of them and was not said yet (issue 195). History and transcript are only
    ever appended to, for the record. Kept under a key of its own, not as an `AgentChannelState`:
    there is no turn and nothing of what we cannot see.
*   **`AgentState`**: the agent itself, across its channels and - for agents of user or global
    scope - across sessions. So far it holds the agent's memory (`agent_memory`, see
    [MEMORY.md](MEMORY.md)).

### The agent state has one owner per process

An agent may run in several channels and sessions at once, each with its own `AgentActor`. The
`RuntimeManager`'s `AgentStates` hands every one of them the same `AgentStateHandle`, so that there
is one owner of the agent's state per process.

A turn does not hold the state. `Turn` receives the handle in its constructor and offers only
generic access: a snapshot (`agent_state().current()`) and staging `AgentStateChange`s
(`stage_agent_change`). It knows nothing of what the state holds; the logic of a kind of state is a
wrapper around the turn, such as `agent_memory::AgentMemory` (recall, remember, forget), which the
component and anything else use.

When the turn completes, `Turn::commit_turn` applies the staged changes to the state as it is at
that moment, under the handle's lock, and stores it. The handle then announces
`GlobalEvent::AgentStateChanged`, with the agent's attributes and without content. No actor thus
overwrites another's changes, and every next turn sees them (issue 62). Numbers for new memory
entries are handed out by the handle at once, since turns in several channels may remember at the
same time.

### One writer per state

Every kind of state has exactly one writer:

*   `ChannelState`: its `ChannelLoop`;
*   `AgentChannelState`: the `AgentActor` of that agent and channel;
*   `ExternalAgentChannelState`: the `ExternalAgentActor` of that agent and channel;
*   `AgentState`: its handle.

This holds because a session starts only once, however many requests ask for it at the same time.
`SessionManager::open_session` holds a lock per session id while it starts one, and `stop_session`
and `delete_session` take the same lock, so a stop cannot miss a start under way, and a deleted
session cannot be started again from the store meanwhile. The JSON stores additionally write every
file through a temporary file of its own.

### Deleting state

The entity store and the state store are configured separately and may be different stores. Deleting
a channel therefore deletes its state explicitly, through `StateStore::delete_channel_state`, which
removes the channel's state and that of every agent in it, external ones included; deleting a
session does the same for each of its channels. It is never a side effect of
`EntityStore::delete_session`.

## Controllers

The live state of the entities of a running session is held by controllers in
`caelumora_runtime::controller`: `SessionController`, `ChannelController`, `AgentController` and
`UserController`, made by the `ControllerManager`. The model only defines the documents; keeping
them live, writing them and announcing changes is the runtime's (issue 86).

*   **Live state.** A controller holds its entity in an `ArcSwap`, for atomic, lock-free reads of
    the current value.
*   **One writer.** A controller is the one writer of its document. It holds a write lock while it
    changes the document, stores the change first - which is where the permission is checked - and
    only then takes it over live and announces it.
*   **Events.** Controllers publish their changes on the `EventBus` (see [EVENTS.md](EVENTS.md)) and
    take over changes announced elsewhere through `process_global_event`.
*   **One shape.** Every controller is built from the document, the event publisher and the store.
    The `UserController` is the exception: users are not changed through it, so it has no store and
    announces nothing.

### Session-owned and per-instance controllers

Sessions and channels belong to a session; agents and users do not.

A `SessionController` and its `ChannelController`s are made with the running session and live as
long as it does.

An agent or a user takes part in any number of sessions. The `ControllerManager` therefore keeps
**one controller per agent and per user in the instance**
(`ControllerManager::agent_controller(id)`, `user_controller(id)`, issue 121). It is loaded from the
store, as the runtime, when the first actor needs it, shared by all its actors in all sessions, and
held only as long as one of them holds it (weak references, like the `AgentStates` of issue 62). An
agent thus has one writer per instance. The `new_…_controller(document)` methods, in contrast, make
the controllers a running session owns.

What an actor is called is asked of it too: `actor_name(actor)` answers a user's or an agent's
current name from its controller, and for an actor that is no entity - an approver, the runtime
itself - the name its kind fixes (`ActorId::fixed_name`).

A missing record is `StoreError::NotFound`; a channel then leaves that participant out (see
[CHANNELS.md](CHANNELS.md)).

### Following changes

Neither the actors nor the sessions subscribe to changes of themselves. The `ControllerManager`
takes every announced change to an agent or user into its live controller - once per instance, from
one task started with its first controller. That is the place where changes made in other instances
will arrive (see [Several instances](#several-instances)).

An event carries the resulting value (`NameChanged(name)`), so taking over an instance's own change
again changes nothing. Agent events carry no session: `AgentUpdated(AgentId, …)`.

## Several instances

The server is meant to run in several instances one day ([122](../issues/122-several-instances.md)).
The runtime is cut so that this stays possible:

*   **A session runs in exactly one instance**, with its channels, their actors and their
    controllers. Requests for a session go to the instance that runs it.
*   **Agents and users run in several instances at once.** An agent of user or global scope takes
    part in sessions that run in different instances, and has an actor, a controller and its
    `AgentState` in each of them. Each instance keeps one of each per agent and user. Between
    instances they are kept in step through the global events, carried by an event bus the instances
    share (Redis, Kafka or the like): the `ControllerManager` keeps the agents' and users' documents
    in step, the `AgentStates` their state.
*   **Writes across instances** cannot be serialised by a lock in one process. The shared stores
    will have to refuse a write made on an outdated version, and events will have to carry the
    version they result in, so that an instance skips one that is older than what it has.
