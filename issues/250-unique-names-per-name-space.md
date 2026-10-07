# 250 — Names are not kept unique in their name space

**Status:** open
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
