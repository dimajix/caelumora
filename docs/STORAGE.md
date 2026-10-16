# Storage

Caelumora keeps two kinds of data apart: long-lived **entities** - definitions and identities - in
the entity stores, one `EntityStore<T>` per kind, and frequently changing runtime **state** in the
`StateStore`; preconfigured components have a `ComponentStore` of their own. This document
describes the stores, who writes what, and the controllers that hold the entities of a running
session live. How access to stored entities is checked is in [SECURITY.md](SECURITY.md); which
stores are used is configured in [CONFIGURATION.md](CONFIGURATION.md).

## The entity store

The entity stores (`EntityStore<T>`, in `caelumora-model`) keep the long-lived domain entities:

*   **Personalities**, the definitions of agents, and **session templates**, which sessions are
    made from.
*   **Identities**: agents, sessions, channels and users. A channel is a document of its own, which
    its session lists by id.
*   **User profiles**: what a user says about themselves - introduction, preferred language,
    answers given in advance. One per user, apart from the `User` record (see
    [SECURITY.md](SECURITY.md#ownership) for why).
*   **Workspaces**: a tree of files a user owns and attaches to sessions (see
    [WORKSPACE.md](WORKSPACE.md)). The document names the storage its files lie in; the files are
    no part of it.
*   **Skills**: procedures written down for agents, a kind of the plugin `caelumora-skills` (see
    [SKILLS.md](SKILLS.md)).
*   **Artifacts**: results an agent published, each a user's, with its versions, a kind of the
    plugin `caelumora-artifacts`. Their content lies in a configured artifact storage, not in the
    document.
*   **Groups**: lists of actors an access list names as one subject - a user's teams, and the
    participants of every session and channel (see [SECURITY.md](SECURITY.md#groups)).
*   **The kinds of the plugins** (see [PLUGINS.md](PLUGINS.md)), kept like the
    core's.

Each kind is kept by a store of its own, `EntityStore<T>` (issue
[323](../issues/323-kinds-enumerated-in-the-core.md)), with the same operations for every kind.
There are two implementations: `MemoryEntityStore<T>`, for tests, and `JsonEntityStore<T>`, which
keeps one JSON file per document in the kind's directory, `entities/<kind>/<id>.json`, and holds
the kind's lock for names. `MemoryStore` and `JsonFileStore` open one of them for any kind that is
asked for (`EntityStoreFactory::entity_store::<T>()`), once, beside the runtime state: whoever asks
for a kind again - the core, a plugin - is given the same store. What the configuration opens is a
`StoreBackend` (`AppConfig::open_entity_store`), which gives the stores of the entities, the
component store and the state store, and refuses a kind whose name another kind has. The stores of
the core's kinds are `EntityStores`, which the runtime is given; a plugin asks for the store of its
kind while it registers. A manager is made with `&EntityStores` - or the store of its kind - and
keeps the stores of the kinds it uses as fields, as every service is held.

Every stored type says what it is to its store (`Stored`): its kind, `Stored::KIND`, an `EntityKind`
named by a string (`"skill"`) that is unique among the kinds; its id and the fields a query names;
the kind of entity it lies in, `Stored::Within` - a channel in a `Session`, every other kind of the
core in `NoEntity` -, by which its events name where it lies (issue 335, see
[EVENTS.md](EVENTS.md#the-form-of-an-event)); and, if its kind is `Scoped`, the name that is unique
in its scope (`Stored::unique_name`). The stores take all of it from the type they read and write:
what is asked of the `AccessPolicy` (`ActionType::get(T::KIND, …)`), the name in errors, logs and
the JSON store's directory, and which names to keep unique. A stored type can be copied and
serialized (`Stored` requires it), which is all a backend needs to keep it. A new kind is added
without a change to the stores or to `EntityKind`.

### Artifact content

The content of artifacts lies in the storages configured under `artifacts`, each an
`ArtifactStorage` that puts, reads and deletes a version without asking permissions - a backend of
the plugin `caelumora-artifacts`, made by the factory registered for its `type` while the plugin
registers (see [ARCHITECTURE.md](ARCHITECTURE.md#backends), issue 328). The `ArtifactManager` is the
one writer of artifact documents and the only code that holds the storages: a version's content is
kept before the document says it exists, and removed again if the document cannot be stored; an
artifact is purged with its content, the content first. It knows no workspace: where the content
comes from is its caller's. Artifacts are entities, not state: each has its own owner, permissions,
a name unique in its scope and a trash, and one that is kept outlives the session it was published
in.

### Preconfigured components

Preconfigured components are runtime configuration that personalities and templates refer to by
`kind: "ref"`. The `ComponentStore` keeps them as `StoredComponents` - one or several component
descriptors with the attributes of who may use and change them - with the same operations as
entities, checked the same way. A descriptor inside a personality or template carries no attributes;
it is covered by those of what holds it. The component store is opened where the entity store is.

### Names and scopes

Every entity has a name, and the fields that mean something for it:

*   **What is a scope** - a session, a channel, a user - has a name only.
*   **What lies in a scope** - a personality, a session template, an agent, a workspace, a skill, an
    artifact - has a name and a `Scope`: where it is found by name. The store finds one in exactly
    its scope; from a place, `scope::find_by_name` finds the innermost of a name - in the place's
    scope, in those around it or in the user's own.
*   **What is offered for choosing** - a personality, a session template - has a `description`, a
    catalogue line.

What lies in a session's or a channel's scope is used only within it and goes with it; a user's
scope or the global one only says where a name is found (see [CHANNELS.md](CHANNELS.md#sessions)).
A template asks for each agent in exactly the scope it names and reuses it only if it is of the
personality it names, so an agent's scope decides whether it is reused - one per user, with its
memory - or made anew for every session. A session template lies in no session's or channel's
scope. A workspace lies in a session's scope, its owner's or the global one - never a channel's,
since a session's channels share one - and belongs to a user wherever it lies; so does an artifact.

**Of one kind, a name is unique in its name space**, and the store keeps it so: nothing is created,
renamed, moved or restored under a name another of its kind has there, compared as names are
(`StoreError::NameTaken`). A channel's name space is its session's scope, a user's the global one;
a session's name is not unique.

*   The check reads every one of the kind that is not deleted, also those the caller may not read
    (see [SECURITY.md](SECURITY.md#how-stores-check)). What is deleted holds no name: restoring it
    is checked as creating it is.
*   It is made in the same step as the write, under a lock of the store.
*   Both stores build their writing operations around one core that takes the check as a hook; a new
    named kind is wired to the named variants, and the store suite states the rule for every kind.

**Values with rules.** A text whose content has rules - a `Name`, a `Language`, the free texts
people write for others (`Text<R>`) - has a type made only through its constructor and checked again
when a document is read, so that a stored document that does not fit fails to load (`ValueError`,
reported as `StoreError::Invalid`). A value is checked, never changed: it is kept exactly as
written, and what identifies something is compared by a key made from it. A `Name` is compared by
its ASCII form, regardless of case and of how whitespace runs, so "Zoë" and "zoe" are one name; none
of these types derefs to `str`, which would bypass that. Above the model - loops, controllers,
events, REST requests - such a text is a string as given; the model makes the value where it is
assigned.

### Queries

Every list of the entity store takes a query (`caelumora_model::store::query`) and answers a page:

*   **A filter**, purely declarative, so that a database could push it into a `WHERE` clause. It
    names fields of the kind, each a path into the document as serialized. Values take the forms
    serde writes but are made without it, so that making a filter cannot fail; a tagged value meets
    an object that *has* its key with a matching value - containment, as `@>` in Postgres -, a
    variant alone (`FieldValue::variant`) an object that has its key or the string of a variant that
    holds nothing - as `?` in Postgres.
*   **A name prefix** (`Filter::NamePrefix`) compares a name as names are compared - regardless of
    case, accents and runs of whitespace -, for a search by what one has typed so far.
*   **Scope conditions** are made by the scope (`Scope::within`, `Scope::seen_from`), so that the
    rule lives with `Scope`.
*   **A limit and a cursor**, in the order of the entities' ids. A page cut short hands out the
    cursor to go on with; the page after it may be empty.
*   **Who may read what is asked afterwards**, entity by entity; a limit counts what the caller may
    read.

Both stores take the keys of a kind first and read an entity only when it is its turn; what follows
a page's limit is not read. Without an index, a list without a limit still goes through every entity
of the kind. Every list that narrows what it read says so in its query; what a query cannot say yet
- a time comparison, a lookup by normalized name, a viewer's clearance - stays code at the caller.

### Deleted entities

What a user deletes is kept, marked, and removed for good only by a purge - an administrator's act
or a retention policy's. The kinds that deleting marks implement `Deletable` and carry who deleted
them and when: sessions, channels, agents, users, user profiles, personalities, session templates
and workspaces. A preconfigured component is not marked: it is reached only through what refers to
it, and is deleted for good.

A marked document is not there for the ordinary operations, before anything is asked: not read,
listed, found by name, changed or deleted again - also not by the system, so that no caller has to
remember to leave it out. Its id stays taken, so seeding does not bring back deleted demo data.
Nothing created or changed may carry the mark: only deleting sets it. What was deleted is reached by
operations of its own, each with a permission of its own:

| Operation | asks | does |
|---|---|---|
| `get_deleted`, `list_deleted` | `ReadDeleted` | read the marked documents; what is not marked is not found |
| `restore` | `ReadDeleted`, then `Restore` | takes the mark away and returns the restored document |
| `purge` | `ReadDeleted`, then `Purge` | removes the document for good - only if it is marked, also when it is removed |

For what is classified - a channel, a workspace, a skill, an artifact -, every operation also asks
whether the caller is cleared for it, the deleted alike (issue 322, see
[SECURITY.md](SECURITY.md#how-stores-check)): the store is given a `ConfidentialityPolicy` beside
its `AccessPolicy`.

Purging only what is still marked keeps a purge from removing what was restored meanwhile. Restoring
and purging a session are serialised by the `SessionManager`, under the lock it also deletes and
starts a session with. What the runtime removes for good that is not marked - the channels of a
session being purged, a document whose creation failed - it marks first, in its own name. The state
of what was deleted stays in the `StateStore` until it is purged (see [Deleting
state](#deleting-state)).

### Demo data

Demo data is not part of any binary. The files the configuration's `seed` names hold preconfigured
components, personalities and session templates as the stores keep them, with fixed ids, and the
sections of the plugins - `skills` among them -, each seeded by the plugin that claims it
(`SeedSection`); a section no plugin claims is an error before anything is stored. At startup both
binaries put into the stores whatever they do not have yet, by id, as the system
(`RuntimeManager::seed`): restarting changes nothing, and a document changed in the store stays as
it is.

## The state store

The `StateStore` (in `caelumora-model`) stores the runtime state of channels and agents:

*   **`ChannelState`**: the channel's history.
*   **`AgentChannelState`**: an agent in one channel, of either kind - processing state, history,
    transcript, notes, prompt texts, and the turn under way (see [AGENT-TURN.md](AGENT-TURN.md)).
    An external agent keeps instead what it keeps of the remote side: the remote conversation and
    the tasks not yet ended.
*   **`AgentState`**: the agent itself, across its channels and sessions - so far its memory (see
    [MEMORY.md](MEMORY.md)).

### What a state tells a store

Every state is `Journaled`: it tells what changed since it was last stored, as a sequence of
changes, and replays such changes onto what was stored before. Its changes are one enum per state,
named "state change" on purpose: a change of the state - what was said, noted, remembered - as
against an update of the entity.

*   A growing part records what was added: a chain tells the nodes added since it was last stored
    and its head, the same form the whole chain is stored in (see
    [AGENT-TURN.md](AGENT-TURN.md#the-transcript-as-a-chain)); what is kept once by digest tells the
    digests added. Each node carries its position, and a node is merged only at the position it
    has, or at the one after the last if it is new.
*   A small part - the processing state, the turn under way, the tokens the agent's calls used, an
    external agent's open tasks and the classifications it was given - is recorded whole: it is
    changed in place and bounded by one turn, one exchange, or the labels in use. What would be
    summed over a whole chain is kept as such a part instead, so that a chain is never read whole
    for it.

Replaying is idempotent, and nothing replayed is a change to store again. Every reference between
parts is a digest or an id, resolved when read and never while replaying, so the order across parts
loses nothing.

A put stores the state, whole or only what changed, and marks it stored on success; on failure the
changes stay pending and come with the next put. The `JsonFileStore` keeps a **snapshot** and a
**journal** per state: a put appends its changes to the journal, unless the journal has grown to the
size of the snapshot (and to `journal_floor`), when the state is written as a new snapshot and the
journal emptied. So the bytes written over a state's life are a small multiple of its final size
instead of growing with its square. What a crash leaves is read as it is: a torn last line is
dropped, and a journal that survived its snapshot replays onto it without effect.

### The agent state has one owner per process

An agent may run in several channels and sessions at once, each with its own actor. Every one of
them is handed the same `AgentStateHandle`, so that there is one owner of the agent's state per
process. A turn does not hold the state: it reads a snapshot and stages changes. When it completes,
`Turn::commit` applies the staged changes to the state as it is at that moment, under the handle's
lock, stores it and announces `agent::STATE_CHANGED` without content. No actor thus overwrites
another's changes, and every next turn sees them. Numbers for new memory entries are handed out by
the handle at once, since turns in several channels may remember at the same time.

### One writer per state

Every state has exactly one writer, and its puts never overlap. A store may keep only what changed
since the state was last stored, and what changed is known of the one instance that was stored: a
second writer would have its changes replayed onto what the first stored. A lock in the store would
not help; the rule is the runtime's to keep:

*   `ChannelState`: its `ChannelLoop`;
*   `AgentChannelState`: the actor of that agent and channel;
*   `AgentState`: its handle;

each holding the state under a lock of its own while it changes and stores it.

This holds because a session starts only once, however many requests ask for it at the same time:
starting, stopping and deleting a session take one lock per session id, so a stop cannot miss a
start under way, and a deleted session cannot be started again meanwhile.

Entities have no such rule: two requests may change one at the same time, and the last write wins.
The JSON stores write every file through a temporary file of its own, so that neither write fails
for the other's.

### Deleting state

Deleting a session or a channel keeps its state, hidden with it; purging removes the state too. The
entity store and the state store may be different stores, so a purge deletes the state explicitly -
a channel's with that of every agent in it, an agent's own with its state in every channel. It is
never a side effect of the `EntityStore`.

## Controllers

The live state of the entities of a running session is held by controllers
(`caelumora_runtime::controller`): `SessionController`, `ChannelController`, `AgentController`,
`UserController` and `WorkspaceController`, made by the `ControllerManager`. The model only defines
the documents; keeping them live, writing them and announcing changes is the runtime's. What a
controller changes and what its entity's manager does is in
[ARCHITECTURE.md](ARCHITECTURE.md#managers-and-controllers).

*   **Live state.** A controller holds its entity for atomic, lock-free reads of the current value.
*   **One writer.** A controller is the one writer of its document. It holds a write lock while it
    changes the document, stores the change first - which is where the permission is checked - and
    only then takes it over live and announces it.
*   **Events.** Controllers publish their changes on the `EventBus` (see [EVENTS.md](EVENTS.md)) and
    take over changes announced elsewhere.
*   **One shape.** Every controller holds its document as a `LiveDocument` (issue 358), which does
    all of the above alike for every kind: a change told by the kind's update event
    (`Updatable::apply`, in the model) is applied, stored with when it was changed
    (`Updatable::touch`, `updated_at` where a kind keeps it), taken over, announced - to who reached
    the document as it was classified and as it is - and logged, once, by the controller. What a
    controller does besides - a group that follows the participants, an event of its own, the
    checks of a workspace, which is changed as stored rather than as live - it does between the
    steps of a `Writing`, the change under way. A running session's and a channel's loop only
    forward to their controller what does no more than that. The `UserController` holds no
    `LiveDocument`: the user's record belongs to the system and is only kept current, and their
    profile is created with its first change and announced as it stands, to its user and the
    runtime alone. The `WorkspaceController` also holds the storage its workspace lies in and the
    confidentiality policy, which a new classification is asked of.

### Session-owned and per-instance controllers

A `SessionController` and its `ChannelController`s are made with the running session and live as
long as it does: a session runs once, so there is never a second. An agent or a user takes part in
any number of sessions, and a workspace may be attached to several and is used directly too, so the
`ControllerManager` keeps **one controller per agent, per user and per workspace in the
instance**: loaded when the first one needs it, shared by all that use it, and held only as long as
one of them holds it. An agent thus has one writer per instance. The manager also answers what an
actor is called - an entity's current name, or the fixed name of an actor that is none.

A missing record is `StoreError::NotFound`, which a lookup that expects it may be gone turns into
`None`; a channel then leaves that participant out (see [CHANNELS.md](CHANNELS.md)).

### Following changes

Neither the actors nor the sessions subscribe to changes of themselves - a running session follows
only whether its agents are deleted or restored, which decides which actors run in its channels (see
[CHANNELS.md](CHANNELS.md)). The `ControllerManager` takes every announced change to an agent, a
user or a workspace into its live controller, once per instance - the place where changes made in
other instances will arrive (see [Several instances](#several-instances)). An event carries the
resulting value, so taking over an instance's own change again changes nothing; a workspace is read
again as stored instead, since the announcement of a new access list does not carry it. A text in it
that does not fit the model's value is left out and logged.

## Several instances

The server is meant to run in several instances one day
([122](../issues/122-several-instances.md)). The runtime is cut so that this stays possible:

*   **A session runs in exactly one instance**, with its channels, their actors and their
    controllers. Requests for a session go to the instance that runs it.
*   **Agents and users run in several instances at once**, with an actor, a controller and an
    `AgentState` in each. Between instances they are kept in step through the global events, carried
    by an event bus the instances share.
*   **Writes across instances** cannot be serialised by a lock in one process. The shared stores
    will have to refuse a write made on an outdated version, and events will have to carry the
    version they result in, so that an instance skips one older than what it has.
