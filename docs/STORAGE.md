# Storage

Caelumora keeps two kinds of data apart: long-lived **entities** - definitions and identities - in
the `EntityStore`, and frequently changing runtime **state** in the `StateStore`; preconfigured
components have a `ComponentStore` of their own. This document
describes both stores, who writes what, and the controllers that hold the entities of a running
session live. How access to stored entities is checked is in [SECURITY.md](SECURITY.md); which
stores are used is configured in [CONFIGURATION.md](CONFIGURATION.md).

## The entity store

The `EntityStore` trait (in `caelumora-model`) stores the long-lived domain entities:

*   **Personalities**: static definitions of agent traits.
*   **Templates**: `SessionTemplate`s, which sessions are made from.
*   **Identities**: `Agent`, `Session`, `Channel` and `User` records. A channel is a document of its
    own, which its session lists by id (issue 70).
*   **User profiles**: what a user says about themselves - their introduction (issue 176), the
    language they prefer (issue 56) and their answers to tool calls given in advance (issue 13).
    One `UserProfile` per user, stored under the user's id; see [SECURITY.md](SECURITY.md#ownership)
    for why it is not part of the `User` record.
*   **Workspaces**: a tree of files a user owns and attaches to sessions (issue
    [253](../issues/253-workspace.md)), with one classification for all of it. The document names
    the storage its files lie in; the files are no part of it, and the store does not touch them.
    A session names the workspace attached to it (`Session::workspace`), at most one.

The files of workspaces lie in the configured storages (`workspaces` in
[CONFIGURATION.md](CONFIGURATION.md)), each served by one `TerminalProvider`
(`caelumora_runtime::terminal`), which makes and removes the place of a workspace and hands out its
`Terminal`, the one handle to its files. The `WorkspaceManager` is the one writer of workspace
documents and the only code that holds the providers: a workspace is made with its place, and
purged with it - the place first, so that a place that cannot be removed leaves the document for
the next purge. What a caller writes as a path is made a `WorkspacePath` - relative, without `..` -
before any provider sees it, and a `local` storage follows a symbolic link only where it stays
inside the workspace.

A terminal may have a `Shell`, a capability of its storage (`TerminalProvider::has_shell`): commands
started in the workspace's directory, followed by what they print and killed with everything they
started, each seen only under the scope it was started with - the tool passes the channel. The
`local` provider's shell is opt-in (`shell`): `bash -c` on Caelumora's host, a process group per
command, an environment cleared but for `PATH`, `HOME` (the workspace), `LANG`, `TERM=dumb`,
`NO_COLOR` and `CAELUMORA_WORKSPACE`, the newest 1 MiB of output kept, and a command kept five
minutes after it ended. Commands live with the provider, not with a call or a turn; one still
running when Caelumora ends is not killed by it.

The `open_terminal_shared` provider (`terminal::open_terminal`, issue
[251](../issues/251-open-terminal.md)) keeps a storage in one Open Terminal instance, started
outside Caelumora, at the `mount` it has the storage: a workspace is the directory `<mount>/<id>`
there, made and removed through the instance's `/files` API, which also reads and writes its files;
its shell is the instance's `/execute`, with the channel as `X-Session-Id`. Its key is resolved
through the secret chain on every request, in the system's name - the storage is the operator's, as
the server's own keys are. It behaves as `local` does, with what the instance does differently met
or said: what a path names is read from its directory's list, since the instance has no `stat`;
a directory is checked to be empty before it is removed, since the instance would remove it with
everything in it; a file is written in place, from memory, so a write that breaks off may leave it
half written; offsets count pieces of output, not bytes; no message of the instance is passed on,
since they name paths inside it. Written against Open Terminal 0.14.0, whose API is 0.x: the
requests and answers relied on are in `open_terminal/wire.rs`, pinned by tests against a stand-in.

Preconfigured components are not among them. They are runtime configuration that personalities and
templates refer to by `kind: "ref"`, and the `ComponentStore` keeps them: a `StoredComponent` - the
`ComponentDescriptor` with the attributes of who may use and change it - by its id, with the same
five operations, checked the same way. A descriptor inside a personality or template carries no
attributes; it is covered by those of what holds it. Both implementations serve as component store
too; `AppConfig::create_component_store` opens it where the entity store is, so the components stay
in `components/` of its directory.

Every entity has a name, and the fields that mean something for it (issue 177):

*   **What is a scope** - a session, a channel, a user - has a name only.
*   **What lies in a scope** - a personality, a session template, an agent, a workspace - has a name
    and a `Scope`: where it is found by name (issue [178](../issues/178-scoped-lookup-by-name.md)).
    The store finds one by its `ScopedName`, in exactly its scope (`find_personality_by_name`,
    `find_session_template_by_name`, `find_agent_by_name`, `find_workspace_by_name`). From a place -
    a channel or a session, and who is looking - `scope::find_by_name` finds the innermost of a
    name: in the place's scope, in those around it or in the user's own. A template asks for each
    agent in exactly the scope it names and reuses it only if it is of the personality it names -
    one of another is an error - so an agent's scope decides whether it is reused - one per user,
    with its memory - or made anew for every session. What lies in a session's or a channel's scope
    is used only within it and goes with it; a user's scope or the global one only says where a name
    is found (see [CHANNELS.md](CHANNELS.md#sessions)). A session template lies in no session's or
    channel's scope: `Stored::validate` refuses it, as a kind may refuse what cannot be stored. A
    workspace lies in a session's scope, its owner's or the global one - never a channel's, since a
    session's channels share one - and belongs to a user wherever it lies (issue
    [251](../issues/251-open-terminal.md), decision 13).
*   What is offered for choosing - a personality, a session template - has a `description` as well,
    a catalogue line.

Of one kind, a name is unique in its name space, and the store keeps it so (issue
[250](../issues/250-unique-names-per-name-space.md)). What is `Scoped` says its name and the scope
it lies in: a personality, a session template, an agent, a workspace their own; a channel its name
in its session's scope; a user their username in the global scope. None of it is created, renamed,
moved into another scope or restored under a name another of its kind has there - compared as
names are - and the store answers `StoreError::NameTaken` (REST `409 name_taken`). A session has a
name, but no unique one.

*   The check reads every one of the kind that is not deleted, also those the caller may not read
    (`store_impl::access::name_free`; see [SECURITY.md](SECURITY.md#how-stores-check)). What is
    deleted holds no name: restoring it is checked as creating it is.
*   It is made in the same step as the write. The memory store checks inside its map's lock; the
    JSON store holds a lock per named kind through every write of that kind - within its process,
    the only one that writes its files - and reads the kind's documents only when the name
    changes.
*   Both stores build their writing operations alike: a core that takes the check as a hook
    (`create_with`, `create_file_with`, ...), a plain variant for the kinds without unique names
    and a named one (`create_named_in`, `create_named_file`, ...) that the named kinds call. A new
    named kind is wired to the named variants; the store suite says the rule for every one.
*   Documents stored before the rule that share a name stay as they are - reading never checks -
    until one of them is renamed.

The type of a name says what it is for (`caelumora_model::value::name`):

*   **`Name`** - a name something is or could be looked up by: a channel's, a session's (issue 248),
    a user's `username` (issue [250](../issues/250-unique-names-per-name-space.md)), which a user
    provider such as a directory decides but which is an identity all the same.
    It is checked when it is made or read as a text of its own rule (`NameRule`: one line, not
    empty, not of whitespace alone, no control characters, at most `MAX_NAME_CHARS`), kept exactly
    as written, and compared by its ASCII form (`any_ascii`) regardless of case and of how
    whitespace runs - "Jonathan" and "jonathan" are one name, and so are "Zoë" and "Zoe", in a
    lookup, a map and a key. The form it is compared by is made once, with the name. Its `handle` is
    what it is `@`-mentioned by.
*   **`ScopedName`** - a `Name` with the `Scope` it lies in: what a definition is found by. Written
    into the document as the fields `name` and `scope`, the scope global unless named.

`Name` is one of the values with rules (`caelumora_model::value`, issue
[248](../issues/248-validated-text-fields.md)), with `Language` and the free text people write for
others to read - `Text<R>`, whose rule `R` says how long it may be and whether it is one line
(`Summary`, `Briefing`, `Introduction`, `Description`): a field whose text may only be what its
rules allow has a type that is made only through its constructor, `new`, and checked again when a
document is read, so that a stored document that does not fit fails to load. They share one API -
`FIELD`, `as_str`, `optional` (a missing or empty text is `None`), the conversions to and from a
`String`, a string in the document - and one error, `ValueError`, whose message names the field,
which the stores report as `StoreError::Invalid` and the REST server answers as `400 invalid_value`.
Above the model - in the loops, the controllers, their events and the REST requests - such a text is
a string as it was given; the model makes the value where it is assigned, a controller as it applies
an event (`Session::describe`, `Name::new` for a new name). A value is checked, never changed: it is
kept exactly as written, and what identifies something - a name, a language - is compared by a key
made from it. None derefs to `str`: a `Name` is compared by its folded form, which `&*name == text`
would bypass.

There are two implementations: `MemoryStore`, for tests, and `JsonFileStore`, which keeps one JSON
file per document.

### Deleted entities

What a user deletes is to be kept, marked, and removed for good only by a purge - an administrator's
act or a retention policy's (issues [164](../issues/164-soft-deletion-and-former-members.md),
[07](../issues/07-session-deletion-audit.md)). The stores are generic over `Stored`
(`caelumora_model::removal`), which asks every kind whether it was deleted. The kinds that deleting
is to mark implement `Deletable` and carry `deleted: Option<Removal>` - who deleted it and when:
sessions, channels, agents, users, user profiles, personalities, session templates and workspaces.
The field is written only when set, so documents stored before it load unchanged. A preconfigured
component in the `ComponentStore` is not marked: it is reached only through the personalities and
templates that refer to it, and hidden with them; it is deleted for good.

A document that carries the mark is not there for the five operations, before anything is asked
(`store_impl::access`): it is not read, not listed, not found by name, not changed and not deleted
again - also not by the system, so that no caller has to remember to leave it out. Its id stays
taken: creating it again is a `Conflict`, and so seeding does not bring back deleted demo data.
Nothing created or changed may carry the mark (`Validation`): only deleting is to set it.

`delete_x` sets the mark - by the caller, now - and keeps the document. What was deleted is reached
by four operations of its own, for every deletable kind alike, each with a permission of its own:

| Operation | asks | does |
|---|---|---|
| `get_deleted_x`, `list_deleted_x` | `ReadDeleted` | read the marked documents; what is not marked is not found |
| `restore_x` | `ReadDeleted`, then `Restore` | takes the mark away |
| `purge_x` | `ReadDeleted`, then `Purge` | removes the document for good - only if it is marked, also when it is removed |

A preconfigured component has none of them: `delete_component` removes it for good. The state of
what was deleted stays in the `StateStore` until it is purged (see *Deleting state*).

Purging only what is marked keeps a purge from removing what was restored meanwhile: the
`MemoryStore` removes an entry only if it is still marked when it does, and the `JsonFileStore`
reads the file once more right before - a file cannot be removed on a condition atomically.
Restoring and purging a session are serialised by the `SessionManager`, under the lock it also
deletes and starts a session with. What the runtime removes for good that is not marked - the
channels of a session being purged, which are hidden with it, or a document whose creation failed
- it marks first, in its own name.

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
*   **`AgentChannelState`**: an agent in one channel, of either kind (issue 206) - its processing
    state, history and transcript, its public and private notes, the texts of its prompt parts by
    digest, and the part only its kind keeps:
    *   of an agent run here, the turn under way, if any (processing state, pending tool calls,
        plan, the turn's messages, changes not yet taken over). See [AGENT-TURN.md](AGENT-TURN.md).
    *   of an external agent (issue 179), what it keeps of the remote side (`remote`, a
        `RemoteState`): the remote conversation and the tasks given to it that have not ended, with
        what came of them and was not said yet (issue 195) and the call they came of (issue 205).
        Its history is all it was offered and all it answered, with each answer the reference to
        what its call was given; its transcript what passed its perceive processors (issue 196) -
        on its active path what the remote agent is given with the next call, and as a branch
        each what a call before gave it; notes, prompt
        parts, tool sets and the turn stay empty, and its processing state `Idle`.
*   **`AgentState`**: the agent itself, across its channels and - for agents of user or global
    scope - across sessions. So far it holds the agent's memory (`agent_memory`, see
    [MEMORY.md](MEMORY.md)).

### What a state tells a store

Every state the `StateStore` keeps is `Journaled` (issue 193): it tells what changed since it was
last stored, as a sequence of changes, and replays such changes onto what was stored before. The
changes of a state are one enum with a variant per part of it - `AgentChannelStateChange`,
`ChannelStateChange`, `AgentStateChange` - with "state" in the name on purpose: a change of the
state, what was said, noted, remembered, as against an update of the entity, its name and
composition.

*   A growing part records what was added. A chain - a history, a transcript, a ledger of notes
    or memory - tells a `Stored<E>`: the nodes added, or changed in their place, since it was last
    stored, and its head; the same form the whole chain is stored in, and what `Chain::apply`
    merges into a chain stored before (issue 197, see
    [AGENT-TURN.md](AGENT-TURN.md#the-transcript-as-a-chain)). The texts of prompt parts and the
    sets of tools, kept once by digest (`Kept<V>`), tell the digests added.
*   A small part - the processing state, the turn under way, an external agent's open tasks - is
    recorded whole, at the end of every sequence: it is changed in place and bounded by one turn
    or one exchange.

Replaying is idempotent, and nothing replayed is a change to store again. Order holds within a
part; across parts it is the fixed order of `changes`, which loses nothing, since every reference
between parts is a digest or an id, resolved when read and never while replaying.

A `put_…` takes the state mutably: the store stores it, whole or only what changed, and marks it
stored on `Ok`; on `Err` the changes stay pending and come with the next put. The `MemoryStore`
keeps the whole state. The `JsonFileStore` keeps two files per state:

*   `<key>.json`, the **snapshot**: the state as it was last written whole.
*   `<key>.jsonl`, the **journal**: one line per put since, holding that put's changes.

A put appends a line, unless there is no snapshot yet or the journal has grown to the size of the
snapshot and to `journal_floor` (64 KiB unless configured, see
[CONFIGURATION.md](CONFIGURATION.md)): then the state is written as a new snapshot, atomically, and
the journal emptied. So what a put writes is what changed, and over a state's life the bytes
written are a small multiple of its final size instead of growing with its square. The sizes are
asked of the file system at every put; the store keeps nothing between puts. Loading reads the
snapshot and replays the journal. What a crash leaves is read as it is: a last line torn by a
crash during an append is dropped (an append that fails cuts the journal back to what it was,
so that no torn line is followed by a good one); a journal that survived its snapshot replays
onto it without effect, since replaying is idempotent. A line that is corrupt before the last, or
does not fit the state, is a corrupt state, as a corrupt file is. Every state file written before
the journal is a valid snapshot without one: nothing has to be migrated.

### The agent state has one owner per process

An agent may run in several channels and sessions at once, each with its own `AgentActor`. The
`RuntimeManager`'s `AgentStates` hands every one of them the same `AgentStateHandle`, so that there
is one owner of the agent's state per process.

A turn does not hold the state. `Turn` receives the handle in its constructor and offers only
plain access: a snapshot (`agent_state().current()`) and staging changes to the memory
(`stage_memory`, kept in `TurnState::memory`). The logic of the memory is a wrapper around the
turn, `agent_memory::AgentMemory` (recall, remember, forget), which the component and anything else
use.

When the turn completes, `Turn::commit` applies the staged changes to the state as it is at
that moment, under the handle's lock, and stores it. The handle then announces
`GlobalEvent::AgentStateChanged`, with the agent's attributes and without content. No actor thus
overwrites another's changes, and every next turn sees them (issue 62). Numbers for new memory
entries are handed out by the handle at once, since turns in several channels may remember at the
same time.

### One writer per state

Every state has exactly one writer, and its puts never overlap (issue 215). A store may keep only
what changed since the state was last stored - the `JsonFileStore` appends it to the journal - and
what changed is known of the one instance that was stored, not of another: a second writer, even
one that puts after the first, would have its changes replayed onto what the first stored. A lock in
the store would not help; the rule is the runtime's to keep, and it does:

*   `ChannelState`: its `ChannelLoop`;
*   `AgentChannelState`: the actor of that agent and channel - its `AgentActor`, or its
    `ExternalAgentActor` for an external agent;
*   `AgentState`: its handle, which stores it under its lock;

each holding the state under a lock of its own while it changes and stores it.

This holds because a session starts only once, however many requests ask for it at the same time.
`SessionManager::open_session` holds a lock per session id while it starts one, and `stop_session`
and `delete_session` take the same lock, so a stop cannot miss a start under way, and a deleted
session cannot be started again from the store meanwhile.

Entities have no such rule: two requests may change one at the same time, and the last write wins.
The JSON stores write every file through a temporary file of its own, so that neither write fails
for the other's (issue 62).

### Deleting state

Deleting a session or a channel keeps its state: it is only marked (issue 164), and its state is
hidden with it. Purging it removes the state too. The entity store and the state store are
configured separately and may be different stores, so a purge deletes the state explicitly:
`StateStore::delete_channel_state` removes a channel's state and that of every agent in it,
external ones included, and purging a session does so for each of its channels - those it listed
and those deleted from it before. `StateStore::delete_agent_state` removes an agent's own state and
its state in every channel, for the purge of an agent. It is never a side effect of the
`EntityStore`.

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
    The `UserController` holds the user's record and their profile: the record belongs to the
    system (issue 94) and is only kept current, the profile is written through it (issue 13) -
    `PUT /profile` goes there, and it announces the profile as it now stands
    (`UserProfileUpdated`), which only its user and the runtime receive.

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

A missing record is `StoreError::NotFound`, which a lookup that expects it may be gone turns into
`None` with `StoreResultExt::optional` (issue 192); a channel then leaves that participant out (see
[CHANNELS.md](CHANNELS.md)).

### Following changes

Neither the actors nor the sessions subscribe to changes of themselves. The `ControllerManager`
takes every announced change to an agent or user into its live controller - once per instance, from
one task started with its first controller. That is the place where changes made in other instances
will arrive (see [Several instances](#several-instances)).

An event carries the resulting value (`NameChanged(name)`), so taking over an instance's own change
again changes nothing. A text in it is a string, as it was given and as the model keeps it, made
into the model's value as the event is applied; one that does not fit there is left out and logged
(issue 248). Agent events carry no session: `AgentUpdated(AgentId, …)`.

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
