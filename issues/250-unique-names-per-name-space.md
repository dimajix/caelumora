# 250 — Names are not kept unique in their name space

**Status:** done
**Area:** Persistence

## The problem

Several kinds are found by name, and a name is meant to identify one of its kind in the place it
lies in. Which place that is differs by kind:

| Kind | Name space | Found by name in |
|---|---|---|
| channel | its session | `SessionLoop::channel_by_name`, the session's view |
| agent, personality, session template | its `Scope` (issue [177](177-metadata-on-some-entities.md)) | a template's agents (`caelumora-runtime/src/template.rs`), issue [178](178-scoped-lookup-by-name.md) |
| user | everywhere | `EntityStore::find_user_by_name` |

Nothing keeps that rule as a whole. Where it is kept at all, it is checked by hand - by whoever
creates or restores, before storing, and without a lock:

- **Channels.** `SessionLoop::create_channel` and `restore_channel` refuse a name the session
  already has (`SessionError::DuplicateChannelName`). Renaming does not: `ChannelLoop::update_name`
  stores whatever name it is given, so two channels of one session can end up with one name - and a
  lookup by name then finds whichever comes first. The check and the channel's entry into the
  session are several awaits apart, so two channels of one name created at once can both pass it.
- **Agents.** Renaming (`AgentController::update_name`) checks nothing, nor does creating; a
  template looks for an agent of the name and makes one if there is none, which two sessions made
  at once can both do. Only `SessionManager::restore_agent` refuses a name taken meanwhile - with
  `StoreError::Conflict`, where a channel answers with an error of its own.
- **Personalities and session templates.** Not checked anywhere.
- **Users.** The store checks a new user's id, not their username, though the username is an
  identity (issue [93](93-entity-store-dialects.md)); a username changed elsewhere
  (`UserUpdateEvent::NameChanged`) is taken over unchecked.

No route of the REST API renames a channel or an agent yet, so nobody can do it today; but the
methods are public, and the web UI or a tool that renames would inherit the gap.

Beside it, `Scope::ChannelScope { session_id, channel: String }` refers to its channel by name: once
a channel can be renamed, whatever lies in its scope loses it. Nothing uses `ChannelScope` yet;
issue [178](178-scoped-lookup-by-name.md) makes it an id.

## Considerations

- **One rule, kept by the store.** "Of one kind, a name is unique in its name space" is a rule of
  what is stored, and only the store can keep it without a race: checked in the same write that
  stores, on create, update and restore. Renaming is then covered without anybody having to think
  of it - checked by hand at the callers, every new path that changes a name is a new gap, as
  renaming a channel showed.
- **What a kind says.** Each stored kind whose name is unique says its name space and its name -
  through `Stored` or a trait beside it. Names are compared by `Name`'s key (issue
  [248](248-validated-text-fields.md)), so "Main" and "main" are one name.
- **A channel's name space is its session.** It needs no scope of its own: its `session_id` says
  it, as `Scope::SessionScope` would.
- **What is deleted holds no name.** Today restoring refuses a name taken meanwhile, so a deleted
  one does not keep its name; the store then asks this when restoring, too.
- **The JSON store holds nothing between check and write.** `create_file` asks whether the document
  exists and writes it in two steps. Checking a name means reading the kind's other documents in
  the same step, under a lock per kind (or per name space). A backend with a unique index (issue
  [03](03-postgres-entity-store.md)) does it itself.
- **One error.** A name that is taken answers the same for every kind, and the REST API answers it
  as `409`; `SessionError::DuplicateChannelName` and the `Conflict` of `restore_agent` become it.
- **Templates' agents.** Making an agent that another made meanwhile then fails; the template looks
  again. *Which* agent it finds is issue [178](178-scoped-lookup-by-name.md)'s.
- Not here: looking up by name across the scopes around a place, shadowing, the personality of a
  template's agent and the lifetime of what lies in a scope - all issue
  [178](178-scoped-lookup-by-name.md).

## Done when

- Creating, renaming or restoring a channel, agent, personality, session template or user under a
  name another of its kind already has in its name space is refused by the store, with one error,
  and the store suite says so for both stores.
- The checks by hand in `create_channel`, `restore_channel` and `restore_agent` are gone.

## Decisions

Taken on 2026-10-07, before the plan below:

- **A username is a `Name`.** Usernames are compared as names are (issue
  [248](248-validated-text-fields.md)), regardless of case and accents. A directory that offers two
  usernames that differ only so offers two logins easily mistaken for one another; enterprise logins
  are stable identifiers anyway, not people's natural names. This takes back what issue 248 said:
  that a user's name stays an opaque `String`.
- **What a kind is named by is a `ScopedName`, said through `Scoped`.** The trait of issue
  [178](178-scoped-lookup-by-name.md) stays and is implemented by every kind whose name is unique:
  a personality, a session template and an agent say their own; a channel says its name in
  `SessionScope { session_id }` - its session is its name space, no scope is stored for it; a user
  says its username in `GlobalScope`. A kind either always has a unique name or never: a session
  has a name but no unique one, a user profile is its user's, a component is known by its id -
  they do not implement `Scoped`. `fn scoped_name(&self) -> Option<ScopedName>` on `Stored`, `None`
  by default, was weighed and left: it would ask of every value what is a fact of its kind. The
  price is in the stores, which are generic over `Stored`: the operations that write get the check
  as a hook, and the named kinds go through variants of their own (see *The stores*). A named kind
  wired to the plain variants would lose the check unnoticed - as a forgotten `scoped_name` would
  have; the store suite checks every named kind.
- **Sessions are not unique by name.** A user may have many sessions of one name.
- **Checked against everything of the kind** - also what the caller may not read, otherwise the
  name would not be unique. Who is refused so learns that the name is taken in that name space, not
  by what or whose. What is deleted holds no name; restoring is checked as creating is.
- **One error:** `StoreError::NameTaken { entity }`, without the name, which the caller has. The
  REST API answers it `409 name_taken`. `SessionError::DuplicateChannelName` (`channel_exists`)
  goes; `StoreError::Conflict` means only an id that exists. The web UI reads neither code.
- **The JSON store locks per kind,** within its process, from reading the kind's documents to
  writing - and only when the name changes. Nothing locks across processes, which the JSON store
  does not support anyway.
- **A template's agent:** if making it fails with `NameTaken`, the template looks once more and
  applies issue 178's rule to what it finds.

## Plan

### What changes

| Where | What |
|---|---|
| `caelumora-model/src/scope.rs` | `Scoped::scoped_name` returns an owned `ScopedName`. |
| `caelumora-model/src/agent.rs`, `template.rs`, `channel.rs`, `user.rs` | `Channel` and `User` implement `Scoped`. `User::username` becomes a `Name`. |
| `caelumora-model/src/value/name.rs` | The module's doc: a user's name is a `Name`, too. |
| `caelumora-model/src/user_provider_impl/accept_any.rs`, `config.rs` | A username that is no name is refused; the configured roles are looked up by `Name`. |
| `caelumora-model/src/store.rs` | `StoreError::NameTaken { entity }`; the trait's doc says the rule. |
| `caelumora-model/src/store_impl/access.rs` | `name_free`, the check. |
| `caelumora-model/src/store_impl/memory.rs`, `json.rs` | A hook for the check in `create`, `update` and `restore`; their variants for the named kinds; a lock per named kind in the JSON store. |
| `caelumora-model/src/store_impl/suite.rs` | The rule for every named kind, in both stores. |
| `caelumora-runtime/src/scoped.rs` | Calls `scoped_name()` as it now returns a value. |
| `caelumora-runtime/src/session.rs` | The checks by hand in `create_channel`, `restore_channel`, `restore_agent` go; `SessionError::DuplicateChannelName` goes. |
| `caelumora-runtime/src/template.rs` | `agent` looks once more after `NameTaken`. |
| `caelumora-rest-server/src/error.rs` | `NameTaken` is `409 name_taken`; `channel_exists` goes. |
| `docs/STORAGE.md`, `docs/CHANNELS.md`, `docs/SECURITY.md`, `docs/REST-SERVER.md` | The rule, its error, what a refusal tells. |

### The model: what a kind is named by

```rust
/// What lies in a scope and is found there by its name, unique of its
/// kind in that scope (issues 178, 250).
pub trait Scoped {
    fn scoped_name(&self) -> ScopedName;
}
```

An owned `ScopedName`, no longer a reference: a channel and a user make theirs when asked, from
`name` and `session_id` and from `username`, so that nothing redundant is stored and nothing can
disagree. A clone costs nothing beside reading the documents.

- `Personality`, `Agent`, `SessionTemplate` return a clone of their `name`, as today by
  reference. `Channel` returns `ScopedName { name, scope: SessionScope { session_id } }`, `User`
  `ScopedName { name: username, scope: GlobalScope }`.
- `scope::find_by_name`, `scoped.rs` (`lying_in`, `deleted_with`) and the stores' `find_in` /
  `find_dir` keep their bound `Scoped` and compare the value.
- `User::username: Name`. `find_user_by_name(ctx, &str)` keeps taking a string and compares it as
  a `Name` does (`Name == str`). `AcceptAnyUserProvider::authenticate` refuses a username that is
  no name (`InvalidCredentials`); its roles are looked up by `Name`, so that "ALICE" gets alice's.

### The stores: the check

One function beside `present` in `store_impl/access.rs`, used by both stores:

```rust
/// Refuses `value` if another of `others` - not `value` itself, not one
/// deleted - has its name.
pub(crate) fn name_free<T: Stored + Scoped>(value: &T, others: impl IntoIterator<Item = &T>, is_self: impl Fn(&T) -> bool, kind: &Actions) -> StoreResult<()>
```

(whether `is_self` is a closure or the store compares keys is decided in step 2). It is asked on
`create` always, on `update` when the name or its scope differs from what is stored, on `restore`
always, against what is present.

The operations that write are generic over `Stored` and stay so; the check has to run inside them,
under the lock that writes. So each store's `create`, `update` and `restore` get a hook - the check
of the kind's other documents, a closure - and two entry points each:

- the plain ones as today (`create_in`, `update_in`, `restore_in` and `create_file`, `update_file`,
  `restore_file`), which pass a hook that checks nothing - for sessions, user profiles and
  components;
- named ones (`create_named_in`, `update_named_in`, `restore_named_in` and their `_file`
  counterparts) with `T: Stored + Scoped`, which pass `name_free` - for personalities, session
  templates, agents, channels and users. Their `EntityStore` methods call these.

How the hook is shaped - a closure over the kind's documents, or the core function taking an
`Option` of it - is decided in step 2; what matters is that the plain and the named ones share one
core and differ only in it.

- **Memory store:** the hook runs inside the map's write lock, together with the id's check and the
  insert (`create`), the write (`update`) and taking the mark away (`restore`). The read of what is
  stored and the access check stay before it, as they are.
- **JSON store:** `JsonFileStore` gets one `tokio::sync::Mutex<()>` per named kind, reached through
  the kind's `EntityKind`. The named variants hold it through the whole write, from reading what
  is stored to writing, so that whether the name changes is decided under it; they read the
  directory (`read_all_from_dir`) only when it does. An update that keeps its name reads nothing
  more than today.
- The check reads every document of the kind, including those the caller may not read, without
  asking the `SecurityManager`: it reads no content out, only whether a name is taken.

### The runtime

- `SessionLoop::create_channel`: the check by hand goes; the store refuses when creating the
  channel's document, before anything else is changed. `restore_channel`: likewise, in
  `store.restore_channel`.
- `SessionManager::restore_agent`: the check by hand - a listing as the system - goes.
- `SessionError::DuplicateChannelName` goes; the docs of the three methods name `NameTaken`.
- `template::agent`: `create_agent` failing with `NameTaken` sends it back to the lookup once; what
  it finds then is reused or refused as issue 178 says. If it finds nothing - the agent lies where
  the caller may not read it - the `NameTaken` stays.
- The cascade (`restore_scoped_in`): an agent whose name was taken meanwhile stays deleted, logged
  with `warn!` like any other failure of the cascade.
- `ChannelLoop::update_name`, `AgentController::update_name` need nothing: the store refuses.

### The REST server

`ApiError`: `StoreError::NameTaken` is `409` with the code `name_taken`, for every kind;
`SessionError::DuplicateChannelName` and `channel_exists` go. `StoreError::Conflict` stays
`already_exists`.

### Steps

1. **Restructure, without a change of behaviour:** `Scoped::scoped_name` returns a value,
   `Channel` and `User` implement `Scoped`, `username` is a `Name`, the user provider and its
   roles. All tests pass as they are, apart from those that build a `User`.
2. **The rule in the stores:** `NameTaken`, `name_free`, the hook and the named variants in both
   stores, the JSON store's locks; the store suite for every named kind - creating, renaming,
   moving to another scope, restoring; a deleted one holds no name; an update that keeps its name
   passes; what the caller may not read counts, too.
3. **The runtime and the REST server:** the checks by hand and `DuplicateChannelName` go; the
   template looks again; `name_taken`. Tests: renaming a channel to a name of its session is
   refused, a channel of that name in another session is not; the template's second look.
4. **Documents** and this issue's history.

### To watch

- **Tests that make two of one name.** Tests that create several agents, personalities or users
  of one name in one scope - helpers that always say "helper", "alice" - fail from step 2 on and
  get names of their own.
- **Data stored before.** Documents that already share a name stay as they are; reading never
  checks. The next write that changes the name of one of them is checked.
- **A probe for names.** A refusal tells that a name is taken in a name space. In `GlobalScope`
  that is a little more than a caller could read; SECURITY.md says so.
- **`find_user_by_name`** finds "ALICE" as alice from now on, in every provider that goes through
  the store.
- **`UserUpdateEvent::NameChanged`** keeps carrying a string, as every event does; the
  `UserController` makes it a `Name` when it takes the change over, and leaves out one that is
  none, as the `SessionController` does.

## History

- **2026-10-07** — Opened while issue [248](248-validated-text-fields.md) made `update_name` take a
  string: creating and restoring a channel check its name against the session's, renaming does not.
- **2026-10-07** — Widened at the maintainer's request, file renamed from
  `250-channel-rename-without-duplicate-check.md`. Opened as "A channel can be renamed to the name
  of another channel of its session"; the description then said that the check belonged to the
  `SessionLoop`, covered by "the session's lock for creating channels". Looking for how other kinds
  do it showed that there is no such lock, that no kind keeps its names unique when renamed, agents
  and users not even when created, and that issue [178](178-scoped-lookup-by-name.md) asked the same
  of definitions ("names are unique per scope"). One rule for every kind, kept by the store, was
  preferred over a check and a lock in the `SessionLoop` that the rule would replace; the uniqueness
  of definitions moved here from issue 178.
- **2026-10-07** — Issue 178 goes first: it says in the model what a place and a name space are -
  `Scope`, with `ChannelScope` by id - and this issue then keeps those name spaces unique in the
  store. `ChannelScope` by id moved there.
- **2026-10-07** — Decisions taken and a plan written (see *Decisions* and *Plan*): usernames are
  names, what a kind is named by is said through the trait `Scoped` of issue 178, which channels
  and users implement too, and the store refuses a name that is taken with `StoreError::NameTaken`,
  through variants of its writing operations for the named kinds. `Stored::scoped_name` returning
  an `Option` was weighed and left.
- **2026-10-07** — Step 1: `Scoped::scoped_name` returns a `ScopedName` of its own, `Channel` and
  `User` are `Scoped`, `User::username` is a `Name`; `AcceptAnyUserProvider` refuses a username
  that is no name and finds the configured roles as names are compared. Unlike the plan said at
  first, `UserUpdateEvent::NameChanged` keeps a string, as every event does: the `UserController`
  makes it a `Name` when it takes it over. No change of what is stored or refused yet.
- **2026-10-07** — Step 2: `StoreError::NameTaken`, `name_free` in `store_impl/access.rs`, and in
  both stores the writing operations as a core with a hook (`…_with`), a plain variant and a named
  one for the five `Scoped` kinds. The memory store checks inside the map's write lock, excluding
  the entity's own key; the JSON store holds a `tokio::sync::Mutex` per named kind
  (`caelumora-model` takes tokio's `sync`). Unlike the plan said at first, a named write holds the
  lock also when the name stays: whether it changes is decided under the lock, else two renames of
  one entity to one name could find each other. The store suite says the rule for every named kind in both stores,
  and that what the caller may not read counts too; a test of the JSON store has sixteen writers
  create one name at once. Two runtime tests that made two personalities or channels of one name
  got names of their own. `AcceptAnyUserProvider`: of two first logins under one name, the store
  now refuses the second.
- **2026-10-07** — Step 3: the checks by hand in `SessionLoop::create_channel`, `restore_channel`
  and `SessionManager::restore_agent` are gone, and with them `SessionError::DuplicateChannelName`;
  the store answers `NameTaken`, which the REST API answers `409 name_taken` - found anywhere in
  the error's chain - instead of `channel_exists` and `already_exists`. `template::agent` looks
  once more after `NameTaken` (`existing_agent`) and keeps the error if it finds none it may read.
  `restore_scoped_in` leaves one whose name was taken meanwhile deleted, with a `warn!`, and
  restores the others - with `?`, as before, the first taken name would have left the rest of the
  scope deleted. Tests: renaming a channel to a name of its session; the template's second look,
  deterministic through a `SecurityManager` that has another make the agent when asked whether the
  template may; an agent the caller may not read keeps its name; a session restored after a name of
  its scope was taken. That a channel's name is free in another session the store suite says.
  REST-SERVER.md names `name_taken`.
- **2026-10-07** — Done. Step 4: STORAGE.md says the rule - what is `Scoped` and its name space,
  that the check reads every one of the kind, that it is made in the same step as the write, how
  both stores build their writing operations, and that names shared before stay until renamed;
  CHANNELS.md that a channel's name is unique in its session, when it is created, renamed or
  restored, and that one whose name was taken stays deleted when its session or channel is
  restored; SECURITY.md what a refusal tells of what the caller may not read; ARCHITECTURE.md that
  a template looks for its agent once more if another made it meanwhile.
