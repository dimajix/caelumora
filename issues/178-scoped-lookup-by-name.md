# 178 — Definitions cannot be looked up by name within their scope

**Status:** done
**Area:** Persistence
**Summary:** Definitions cannot be looked up by name within their scope, and the one lookup that uses a scope - a template's agents - finds too much and ignores the personality.

## The problem

`Scope` was meant as a name space: a personality, a session template or an agent lies in a scope - a
channel, a session, a user, or everywhere - and a lookup by name from some place sees what lies in
that place and in the places around it, the innermost first. A user who adds an agent to a channel
would be offered the personalities of that channel, of its session, their own and the global ones; a
personality of one session would exist nowhere else.

None of this is built:

- **No lookup.** Nothing looks a definition up by name with that hierarchy; `list_personalities`
  returns everything the caller may read.
- **The one check there is goes the wrong way.** A template finds an existing agent with
  `scope.contains(&agent.scope)` (`caelumora-runtime/src/template.rs`, `agent`): whether the
  *requested* scope contains the agent's. A request in `GlobalScope` therefore matches every agent
  of that name, the session agent of another user included, if the caller may read it. A lookup from
  a place needs the converse: whether the definition is visible from there.
- **The personality is ignored.** The same lookup finds an agent by name and scope only: a template
  that names "Jonathan" with another personality gets the old Jonathan with the old personality.
- **No uniqueness.** Two definitions of one name in one scope can be created - now issue
  [250](250-unique-names-per-name-space.md), which keeps names unique for every kind, not only for
  definitions.
- **No lifetime.** What lies in a session's or a channel's scope ought to go with it.

## Why it matters

Without it, every definition is global in effect, and scope is a field that only templates' agents
read - in a way that finds too much.

## Considerations

- **Scope is not ownership.** The owner (`SecurityAttributes`) decides who may change a definition,
  the scope where it can be found by name. A global personality belongs to the system; a personality
  of one session may belong to the session's owner and still be offered only there. Whether a caller
  may read what the lookup finds is asked as always.
- **Where the question lives.** "Visible from this place" is a question of the model (`Scope`),
  answered with the place's chain (channel, session, user, global); the store lists, the caller
  filters - or the `EntityStore` gets a lookup of its own, if a backend can do better (issue
  [03](03-postgres-entity-store.md)).
- **Shadowing.** An inner definition hides an outer one of the same name.
- **Reuse of agents.** A template's agent in `UserScope` is the one agent of that name and
  personality of the user, with its memory; in `SessionScope` a new one per session. That rule
  should be the lookup's, not a second one beside it.

## Done when

- A definition can be looked up by name from a place, seeing its own scope and the ones around it,
  innermost first.
- Templates find their agents through it, by personality as well as name.
- Definitions of a session's or a channel's scope go with it.

## Decisions

Taken on 2026-10-07, before the plan below:

- **Whose user scope.** The chain of a place is channel, session, user, global - and the user in it
  is who is looking (`SecurityContext::initiator_id`), not the session's owner. Whoever makes a
  session from a template is its owner, so both are one there; whoever later adds an agent to
  somebody else's channel is offered their own personalities, not the owner's.
- **A template's agent.** The rule falls out of the lookup and the scope the template asks for: the
  agent of the name is looked up from the new session; it is reused only if it lies in the
  requested scope *and* has the requested personality; one that lies further out is shadowed by a
  new one in the requested scope; one that lies in the requested scope with another personality
  is an error, neither reused quietly nor replaced.
- **Lifetime** stays in this issue, as its last step: deleting a session or a channel deletes the
  definitions of its scope with it, restoring restores them, purging purges them.
- **Not in this issue:** a REST parameter that lists definitions from a place; the web UI does not
  ask for one yet. `GroupScope` is visible from no place and otherwise left to issue
  [23](23-acls-in-security-attributes.md).
- **Order with issue [250](250-unique-names-per-name-space.md):** this issue first. It says in
  the model what a place and a name space are; 250 then keeps those name spaces unique in the
  store. Until it does, a lookup that finds two of one name in one scope takes the first listed.

## Plan

### What changes

| Where | What |
|---|---|
| `caelumora-model/src/scope.rs` | `Scope::ChannelScope` refers to its channel by `ChannelId`, not by name. `Scope::depth`, `Scope::visible_from`. The trait `Definition` and the lookup `find_by_name` / `visible_from` over any list of definitions. |
| `caelumora-model/src/agent.rs`, `template.rs` | `Personality`, `Agent` and `SessionTemplate` implement `Definition`. `TemplateError::AgentNameTaken`. |
| `caelumora-runtime/src/template.rs` | `agent` looks the agent up through the model's lookup, from the new session, and applies the reuse rule. |
| `caelumora-runtime/src/session.rs` | `SessionManager::delete_session`, `restore_session`, `purge_session` and `SessionLoop::delete_channel`, `restore_channel` take the definitions of their scope along, through three functions beside `purge_channel_document`. |
| `caelumora-runtime/tests/security_suite.rs` | A session made from a template finds only what its maker may read; deleting, restoring and purging a session takes the agents of its scope along, not those of the user's. |
| `docs/STORAGE.md`, `docs/CHANNELS.md`, `docs/ARCHITECTURE.md` | The definitions section, deleting a session, the template's lookup. |

Nothing changes in `EntityStore`, the REST server or the web UI.

### The model: what a place sees

`Scope` keeps `contains` - whether `self` is the scope `other` or one around it - and gets:

- `fn depth(&self) -> u8`: how far in it lies - `GlobalScope` 0, `UserScope` and `GroupScope` 1,
  `SessionScope` 2, `ChannelScope` 3. The order a lookup prefers: innermost first.
- `fn visible_from(&self, place: &Scope, user: Option<UserId>) -> bool`: whether a definition in
  `self` is found from `place` by `user` - `self.contains(place)`, or `self` is the user's own
  scope. That is the whole chain: a channel's scope is contained by its session's and by the global
  one, the user's scope is the second axis, what is looking. `GroupScope` contains only itself and
  is never a place, so nothing in it is found.

A place is the innermost scope of where one looks from and who is looking: a `Scope` and an
`Option<UserId>`, which the caller has in its `SecurityContext`. No type of its own for the two -
there are two callers, the template and later the views - until a third makes one worth it.

The definitions are what has a `ScopedName`: `pub trait Definition { fn scoped_name(&self) ->
&ScopedName; }`, implemented by `Personality`, `Agent` and `SessionTemplate`. Issue 250 will need
the same of every kind with a name space, and may widen it. Over it, in `scope.rs`:

- `fn visible<'a, D: Definition>(definitions: impl IntoIterator<Item = &'a D>, place: &Scope, user: Option<UserId>) -> impl Iterator<Item = &'a D>`:
  what is found from a place, as the web UI will one day offer it.
- `fn find_by_name<'a, D: Definition>(definitions: impl IntoIterator<Item = &'a D>, name: &Name, place: &Scope, user: Option<UserId>) -> Option<&'a D>`:
  of those of `name` (compared as `Name` compares, issue 248), the one that lies innermost
  (`max_by_key(depth)`); of two at one depth, the first listed, which issue 250 rules out for one
  scope.

The store keeps listing as it does, and lists only what the caller may read; the caller filters.
A store that can look up by itself (issue [03](03-postgres-entity-store.md)) may get a method
later.

### The runtime: a template's agent

`resolve_participants` already has the session's id and owner; `agent` gets the place from it -
`SessionScope { session_id }` and the owner - and does, for a request of `name` in scope `S` with
personality `P`:

1. `store.list_agents(context)` as today: a refused or failed listing is an error, never a reason
   for a second agent (issue 77).
2. `find_by_name(agents, name, place, Some(owner))`.
3. Found, `scope == S`, `personality_id == P`: reuse it - the user's agent with its memory, or a
   global one.
4. Found, `scope == S`, another personality: `TemplateError::AgentNameTaken { name, scope }`,
   which the REST server answers as it answers every `TemplateError` (`400 template_invalid`).
5. Found further out (a user's "helper" while the template asks for `SessionScope`), or not found:
   `Agent::new` in `S`, owned by the maker, and `store.create_agent`. The new one shadows the outer
   one from this session on.

Today's check, `name.scope.contains(&agent.scope)`, asks the converse and finds too much: a
request in `GlobalScope` reuses any user's agent of that name that the maker may read. Making a
session from such a template will make a global agent instead - a change of behaviour, intended.

### The runtime: what goes with a session or a channel

Three functions beside `purge_channel_document` in `session.rs`, each over the three kinds of
definitions and over a scope:

- `delete_definitions_of(store, state, events, scope)`: lists the kind's documents, takes those
  whose `scope` *is* `scope` (not those around it), and marks them deleted in the runtime's name -
  `SecurityContext::system()`, since deleting what holds the scope was permitted, as
  `purge_session` does for channels. An agent is announced (`GlobalEvent::AgentDeleted`) as
  `SessionManager::delete_agent` announces it; a personality or template has no event and no
  runtime path today, it is marked in the store alone.
- `restore_definitions_of(…)`: the deleted definitions of `scope` whose `Removal::at` is not
  before the holder's are restored - those deleted with it; one deleted by hand before stays
  deleted, as a channel deleted before its session does.
- `purge_definitions_of(…)`: every deleted definition of `scope`, and an agent's state with it
  (`StateStore::delete_agent_state`), as `purge_agent` does.

Callers:

- `SessionManager::delete_session`: after the session is marked, for `SessionScope { id }` and
  for `ChannelScope` of each of its channels. Its agents take part in it alone - a session-scoped
  agent is in no other session - so leaving channels is nothing to do; `restore_session` lists them
  again with the channels.
- `SessionManager::restore_session`, `purge_session`: the same scopes, restored or purged. The
  purge takes a `ChannelScope` of the channels deleted from it before, too, as it takes their
  documents.
- `SessionLoop::delete_channel`, `restore_channel`: `ChannelScope { session_id, channel }`. A
  channel is purged only with its session.

All of it under the locks these methods already hold (`start_lock`); the kinds are deleted after
the holder, so that a restore meanwhile finds nothing half done.

### Steps

1. **Model.** `ChannelScope` by id; `depth`, `visible_from`, `Definition`, `visible`,
   `find_by_name`, with tests in `scope.rs`. No behaviour changes: the template still does what it
   did.
2. **Template.** `agent` through the lookup, with the reuse rule, `AgentNameTaken` and tests in
   `caelumora-runtime/src/template.rs` (reused, shadowed, taken, not found); the security suite's
   template test checks that a maker finds only what they may read.
3. **Lifetime.** The three functions and their callers; tests in `session.rs` and the security
   suite (deleting a session takes its agent along, not the user's; restoring brings it back, with
   its state; purging purges it).
4. **Documents and history.** `STORAGE.md` (definitions), `CHANNELS.md` (deleting a session and a
   channel), `ARCHITECTURE.md` line on the template, `SECURITY.md` if the suite's section lists
   the tests; `## History` here.

### To watch

- **Names compare as `Name` compares** (issue 248): "Helper" finds "helper". The place's scopes
  compare by id.
- **Reading rights are the store's.** The lookup sees only what the listing gave it; it never
  widens what a caller may read, and a refused listing is an error, not "none" (issue 77).
- **Not unique yet.** Two agents of one name in one scope - possible until issue 250 - are
  resolved to the first listed, which `MemoryStore` and `JsonFileStore` order differently. The
  tests do not rely on it.
- **`ChannelScope` by id** changes what is stored for a definition in a channel's scope. None
  exists - nothing writes `ChannelScope` - so nothing is migrated, as decided in issue 248 for old
  data.
- **A session-scoped agent deleted with its session** keeps its state until the session is
  purged (issue 164); the channels keep what it said.
- **`delete_agent` by hand on a session-scoped agent** stays as it is; restoring the session does
  not bring it back (`Removal::at` before the session's).
- **The seed** (`examples/seed.yaml`) uses `SessionScope` and `UserScope` only and keeps working;
  a template that names an agent in `GlobalScope` makes a global one from now on.
- **In whose name.** The cascades run as `SecurityContext::system()`: the store would otherwise
  ask `DeleteAgent` of a maker who may delete the session but, by the policy, not the agent in it.
  Said in the method's documentation, as it is for channels.

## History

- **2026-10-01** — Issue created at the maintainer's request, while dissolving `Metadata` (issue
  [177](177-metadata-on-some-entities.md)), which keeps `scope` on the definitions for this.
- **2026-10-07** — Narrowed at the maintainer's request: "names are unique per scope" was taken out
  of *Done when* and moved to issue [250](250-unique-names-per-name-space.md), which keeps names
  unique in their name space for every kind - channels in their session and users everywhere as
  well as definitions in their scope - in the store, on create, update and restore. Left here: the
  lookup across the scopes around a place, shadowing, the personality of a template's agent, and
  the lifetime of what lies in a scope. Found on the way: `Scope::ChannelScope` refers to its
  channel by name, which renaming would break; it becomes an id here, where the chain of a place
  needs it.
- **2026-10-07** — Decided to do this issue before 250, not after, as the history of 250 first
  said: the model says what a place and a name space are, the store then keeps the name spaces
  unique. The decisions and the plan above were written at the maintainer's request.
- **2026-10-07** — Step 1 of the plan, without a change of behaviour: `Scope::ChannelScope` refers
  to its channel as `channel_id`, a `ChannelId`; `Scope` says how deep it lies (`depth`) and whether
  what lies in it is found from a place by a user (`visible_from`, `GroupScope` from nowhere); the
  trait `Definition` (`scoped_name`) is implemented by `Personality`, `Agent` and `SessionTemplate`;
  `scope::find_by_name` finds a definition of a name from a place, innermost first, the first of two
  equally deep. Unlike the plan, `depth`, `visible_from` and the listing for a place (`visible`)
  are not public: nothing outside the module uses them yet, and the listing comes with whatever
  offers definitions from a place.
- **2026-10-07** — Step 2, differently from the plan. Building it showed that a template does not
  ask what is seen from a place under a name but whether the agent of that name exists in the scope
  it asks for: through `find_by_name` from the new session, a user's own "helper" lies further in
  than a global one and is found instead, so a template asking for a global "helper" would make a
  new global one for every session. Decided with the maintainer: the store finds a definition by
  its `ScopedName` in exactly its scope - `find_personality_by_name`,
  `find_session_template_by_name`, `find_agent_by_name`, the counterparts of `find_user_by_name`,
  `NotFound` if there is none among what the caller may read; tested in the store suite for both
  stores. A template's agent is the one `find_agent_by_name` finds, reused if it is of the
  personality asked for, `TemplateError::AgentNameTaken` if not (`400 template_invalid`), made in
  the scope asked for if there is none; tests in `caelumora-runtime/src/template.rs`, among them
  one global agent for a template asking twice although the user's own lies further in. The
  security suite asks the same as before: the lookup lists the agents and reads what it filters.
  `scope::find_by_name` stays for what looks up from a place - mentioning, adding an agent by name,
  offering what is there - and has no caller in this issue; the second point of *Done when* is
  met with the lookup in exactly one scope. `docs/STORAGE.md` says both.
- **2026-10-07** — Step 3: what lies in a scope goes with what holds it. A new module
  `caelumora-runtime/src/definitions.rs` deletes, restores and purges the personalities, session
  templates and agents that lie in a scope *or within it* (`Scope::contains`, so a session takes its
  channels' scopes along), in the runtime's name, announcing agents as `delete_agent` and
  `restore_agent` do. Restoring takes back only what was deleted at the holder's `Removal::at` or
  later, so that what was deleted by hand before stays deleted. Callers: `delete_session`,
  `restore_session` - which reads when the session was deleted in the runtime's name before the mark
  goes, and uses it only once restoring was permitted - and `purge_session` of the
  `SessionManager`, `delete_channel` and `restore_channel` of the `SessionLoop`. Deleting and
  restoring log a failure of the cascade and go on, since the holder is already changed; purging
  reports it. An agent of a channel's scope that takes part in another channel of the session leaves
  it; for that, `SessionLoop::leave_channels` was taken out of `SessionManager::delete_agent`, which
  now uses it too. Nothing creates a definition of a channel's scope yet (`ScopeTemplate` has no
  channel scope), so that path is covered by tests alone. Tests: `session.rs` (a channel's and a
  session's definitions go and come back, a user's stays, one deleted before stays deleted, purging
  takes them all), and the security suite's `deleting_a_session_needs_owning_it`, which shows that
  bob's agent of alice's session goes with it without anything more being asked of her.
- **2026-10-07** — Step 4 and done. `docs/CHANNELS.md` says what goes with a session and a channel,
  `docs/ARCHITECTURE.md` how a template finds its agents, `docs/SECURITY.md` that the cascade runs
  in the runtime's name and deletes somebody else's agent of the scope too; `docs/STORAGE.md` was
  updated with step 2. Against *Done when*: a definition is looked up from a place, innermost
  first, by `scope::find_by_name`, which has no caller yet; templates find their agents by name,
  personality and scope - in exactly the scope asked for, through the store, not through the lookup
  from a place (step 2); what lies in a session's or a channel's scope goes with it (step 3).
  Uniqueness of names is issue [250](250-unique-names-per-name-space.md), next.
- **2026-10-07** — Extended after review, at the maintainer's proposal: what lies in a session's or
  a channel's scope is used only within it, which makes what goes with it whole. The scope answers
  two questions: where a name is found (the chain with the user, for the lookup and for issue 250),
  and where something reached by its id may be used - only within a session's or a channel's scope
  it lies in; a user's or the global scope restricts nothing, the permissions decide. Built:
  `Scope::admits`; an agent takes part only where its scope admits the channel
  (`Member::require_in_scope`, in `channel::require_admitted` - renamed from `require_cleared`, for
  creating, restoring and making channels - and in `ChannelLoop::add_participant`;
  `ChannelError::OutOfScope`, `400 invalid_participant`); a personality makes agents only where its
  scope admits theirs (`TemplateError::PersonalityOutOfScope`); a session template of a session's
  or a channel's scope is not stored (`Stored::validate`, a new method every stored kind may
  override, checked by the store on create and update as `StoreError::Validation`). Deleting a
  channel no longer has its agents leave other channels: they take part in none.
  `SessionLoop::leave_channels` stays for `delete_agent`. Before, a personality of a session could
  make a global agent that outlived it with a personality that was gone. Tests in `scope.rs`,
  `template.rs`, `session.rs` and the store suite; the security suite is unchanged, since the checks
  ask nothing of the policy. `docs/CHANNELS.md` and `docs/STORAGE.md` say the rule.
- **2026-10-07** — Renamed at the maintainer's request: the trait `Definition` is `Scoped`, beside
  `Secured`, `Stored` and `Deletable`, since an agent is no definition and the trait asks only for
  a `ScopedName`; the runtime's module `definitions.rs` is `scoped.rs`, its functions
  `delete_scoped_in`, `restore_scoped_in` and `purge_scoped_in`. The plan and the entries above
  keep the old names.
