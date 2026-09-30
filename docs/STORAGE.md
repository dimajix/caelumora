# Storage

## Storage and Persistence

Caelumora uses a tiered storage approach to separate static definitions, dynamic identities, and runtime state.

### `EntityStore`
The `EntityStore` trait (in `caelumora-model`) manages the persistence of long-lived domain entities. It handles:
*   **Personalities**: Static definitions of agent traits.
*   **Templates**: `SessionTemplate`s, which sessions are made from.
*   **Identities**: `Agent`, `Session`, `Channel` and `User` records. A channel is a document of its own, which its session lists by id (issue 70).

Implementations include `MemoryStore` (for testing) and `JsonFileStore` (for simple file-based persistence).

**Demo data** (issue 105) is not part of any binary: the files the configuration's `seed` names (`examples/seed.yaml`, seeded by `examples/config.yaml`) hold personalities and session templates as the store keeps them, with fixed ids. At startup both binaries put into the store what it does not have yet, by id (`caelumora_runtime::seed::load`), as the system - so restarting changes nothing, and a document changed in the store stays as it is. Without a `seed` nothing is seeded.

### `StateStore`
The `StateStore` trait (in `caelumora-model`) is dedicated to managing the frequently changing runtime state of agents and sessions.
*   **`ChannelState`**: the channel's history.
*   **`AgentChannelState`**: an agent in one channel - its history and context, its public notes (later also its private notes), the texts of its prompt parts by digest, and its turn state (processing state, pending tool calls, plan, the turn's messages, notes not yet taken over).
*   **`AgentState`**: the agent itself, across its channels and - for agents of user or global scope - across sessions: so far its memory (`agent_memory`). One owner per agent and process: the `RuntimeManager`'s `AgentStates` hands every `AgentActor` of the agent, in whatever channel or session, the same `AgentStateHandle`. A turn does not hold the state; `Turn` gets the handle in its constructor and offers only generic access - a snapshot (`agent_state().current()`), staging `AgentStateChange`s (`stage_agent_change`) - and knows nothing of what the state holds: the logic of a kind of state is a wrapper around the turn, such as `agent_memory::AgentMemory` (recall, remember, forget), which the component and anything else use. `Turn::commit_turn` applies the staged changes to the state as it is when the turn completes, under the handle's lock, and stores, and the handle then announces `GlobalEvent::AgentStateChanged` (with the agent's attributes, without content) - so no actor overwrites another's changes, and every next turn sees them (issue 62). Numbers for new memory entries are handed out by the handle at once, since turns in several channels may remember at the same time.

Every kind of state has exactly one writer: `ChannelState` its `ChannelLoop`, `AgentChannelState` the `AgentActor` of that agent and channel, `AgentState` its handle. That holds because a session starts only once, however many requests ask for it at the same time (`SessionManager::open_session` holds a lock per session id while it starts one, and `stop_session` and `delete_session` take the same lock, so a stop cannot miss a start under way and a deleted session cannot be started again from the store meanwhile); the JSON stores additionally write every file through a temporary file of its own.
*   **`SessionState`**: state of a live session.

Entity store and state store are configured separately and may be different stores. Deleting a session or a channel therefore deletes its state through the `StateStore` (`delete_session_state`, `delete_channel_state`, which removes the channel's state and that of every agent in it), never as a side effect of `EntityStore::delete_session`.


## Controllers and State Management

The live state of the entities of a running session is held by controllers in `caelumora_runtime::controller` - `SessionController`, `ChannelController`, `AgentController`, `UserController`, made by the `ControllerManager`. The model only defines the documents; keeping them live, writing them and announcing changes is the runtime's (issue 86).
*   **Live State**: Controllers use `ArcSwap` to provide atomic, lock-free access to the current state of an entity.
*   **One writer**: a controller is the one writer of its document. It holds a write lock while it changes it, stores the change first - which is where the permission is checked - and only then takes it over live and announces it.
*   **Eventing**: Controllers publish their changes on the `EventBus`, and take over changes announced from elsewhere through `process_global_event`.
*   **One shape**: every controller is built from the document, the event publisher and the store. The `UserController` is the exception: users are not changed through it, so it has no store and announces nothing.
*   **Sessions and channels belong to a session, agents and users do not**: a `SessionController` and its `ChannelController`s are made with the running session and live as long as it does. An agent or a user takes part in any number of sessions, so the `ControllerManager` keeps **one controller per agent and per user in the instance** (`ControllerManager::agent_controller(id)`, `user_controller(id)`, issue 121; the `new_…_controller(document)` methods, in contrast, make the controllers a running session owns) - loaded from the store as the runtime when the first actor needs it, shared by all its actors in all sessions, and held only as long as one of them does (weak references, like the `AgentStates` of issue 62). An agent thus has one writer per instance. A missing record is `StoreError::NotFound`; a channel then leaves that participant out.
*   **Following changes**: the actors do not subscribe to changes of themselves, and neither do the sessions. The `ControllerManager` takes every announced change to an agent or user into its live controller - once per instance, from one task started with its first controller. That is the place where changes made in other instances will arrive (see *Several instances*). An event carries the resulting value (`NameChanged(name)`), so taking over an instance's own change again changes nothing. Agent events carry no session: `AgentUpdated(AgentId, …)`.

### Several instances

The server is meant to run in several instances one day ([122](../issues/122-several-instances.md)); the runtime is cut so that this stays possible:

*   **A session runs in exactly one instance**, with its channels, their actors and their controllers. Requests for a session go to the instance that runs it.
*   **Agents and users run in several instances at once**: an agent of user or global scope takes part in sessions that run in different instances, and has an actor, a controller and its `AgentState` in each of them. Each instance keeps one of each per agent and user; between instances they are kept in step through the global events, which an event bus shared by the instances (Redis, Kafka or the like) carries - the `ControllerManager` for the agents' and users' documents, the `AgentStates` for their state.
*   **Writes across instances** cannot be serialised by a lock in one process. The shared stores have to refuse a write that was made on an outdated version, and events have to carry the version they result in, so that an instance skips one that is older than what it has.
