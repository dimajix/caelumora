# 11 — Agent ownership

**Status:** done
**Area:** Identity & access management

## The problem

`SecurityAttributes` carries an `owner_id` (`caelumora-model/src/security.rs:42`), but nothing
enforces it. Agents, sessions and templates are effectively global: any caller can list, fetch and
update any of them, because `DummySecurityManager` approves everything and no store filters by
owner.

## Why it matters

An agent is a configured, named thing with credentials attached to it — an `mcp_tool` component
pointing at a private server, a personality written for a particular purpose. Sharing all of those
with every user of the system is only tenable while there is one user.

Ownership is also the smallest useful access-control model, and a natural first step after
[10](10-user-authentication.md): most of what the system needs is "you can see and use what is
yours".

## Considerations

- The `ActionType` enum already enumerates every securable operation (`GetAgent`, `UpdateSession`,
  `PutComponent`, …), so a policy has a complete set of hooks to attach to. The work is writing the
  policy, not finding the call sites.
- Listing is the awkward case: `ListSessions` and `FindAgent` return collections, and a policy that
  approves or denies the whole call is the wrong shape. Either the store filters by owner (which
  pushes policy into the store — see the caution in [03](03-postgres-entity-store.md)) or the
  `SecurityManager` gains a way to express a filter.
- Ownership alone will not cover shared agents, teams, or an agent that acts on behalf of several
  users. It does not have to — but the model should not make those impossible to add later.
- Deletion and ownership interact: what happens to an agent when its owner is removed?

## Done when

- Entities record their owner and it is populated from the authenticated caller.
- Reads and writes of entities the caller does not own are denied, including in list results.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-24** — Done, in two commits. Decisions, with what was weighed:
  - **Actions carry `SecurityAttributes`, not ids and not entities.** `GetSession(id)` let no policy
    see the owner. Passing the whole entity was proposed first and rejected: a policy should get
    exactly the data meant for deciding, one shape for every entity kind (the input an OPA policy,
    [24](24-opa-security-manager.md), would want), and a store can fetch the attributes more cheaply
    than the entity. `Get`, `Update` and `Delete` carry the attributes *as stored* - never those of
    a new value, or writing oneself in as owner would pass - and `Create` those of the new entity.
    `Find`/`List` ask whether one may search at all; results are filtered with `Get`. This answers
    the "listing is the awkward case" consideration above without moving policy into the store: the
    store only asks, per entity, through one shared module (`store_impl::access`), and the
    `EntityStore` trait is unchanged.
  - **A read that is refused is "not found"**, so ids cannot be probed; a change or deletion of
    something one may read is refused (`403`).
  - **One place for the owner.** `SecurityAttributes` - on sessions, channels and agents already,
    now also on personalities, session templates and preconfigured components; `Metadata.owner`,
    which duplicated it, is gone. It gained `contributors`: actors that read and use an entity but
    may not change or delete it. Needed because issue 30 lets several users act in one channel - an
    owner-only rule would hide a session from the people writing in it - and the policy sees only
    the attributes, so participants had to be there. Named `contributors` rather than `co_owners`,
    which would promise owner rights. It is the first, small step towards
    [23](23-acls-in-security-attributes.md)'s access lists, without fixing their verbs.
  - **Policy:** `OwnershipSecurityManager`, selected by `AppConfig`'s new `security` section,
    `ownership` by default (`allow_all` for tests and development). Owners do everything,
    contributors read and use, what belongs to the system (or to no one, as everything stored before
    this) everybody reads and uses, nobody creates in someone else's name.
  - **The runtime acting on its own** is `SecurityContext::system()` (`ActorId::System`), set
    deliberately where no user acts: initial data, keeping channels in step with join events, the
    `accept_any` provider's records, resolving the JWT key. `SecurityContext::default()` does not
    become the system - an empty context has no rights.
  - **Asking before acting** also outside the stores: a running session is checked in
    `SessionManager::open_session` like a stored one (before, being in memory bypassed every check),
    `delete_session` checks before it stops anything (before, a refused deletion still stopped the
    session), and renaming stores first and changes the live entity afterwards.

  Not in scope and still open: roles such as an administrator (with
  [23](23-acls-in-security-attributes.md)); what happens to an owner's entities when the owner is
  removed - for now they stay; and the global event stream, which still sends every user every
  session's events - [39](39-global-events-not-filtered.md).
