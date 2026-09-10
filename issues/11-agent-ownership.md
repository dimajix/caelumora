# 11 — Agent ownership

**Status:** open
**Area:** Identity & access management

## The problem

`SecurityAttributes` carries an `owner_id` (`caelumora-model/src/security.rs:42`),
but nothing enforces it. Agents, sessions and templates are effectively global:
any caller can list, fetch and update any of them, because
`DummySecurityManager` approves everything and no store filters by owner.

## Why it matters

An agent is a configured, named thing with credentials attached to it — an
`mcp_tool` component pointing at a private server, a personality written for a
particular purpose. Sharing all of those with every user of the system is only
tenable while there is one user.

Ownership is also the smallest useful access-control model, and a natural
first step after [10](10-user-authentication.md): most of what the system
needs is "you can see and use what is yours".

## Considerations

- The `ActionType` enum already enumerates every securable operation
  (`GetAgent`, `UpdateSession`, `PutComponent`, …), so a policy has a complete
  set of hooks to attach to. The work is writing the policy, not finding the
  call sites.
- Listing is the awkward case: `ListSessions` and `FindAgent` return
  collections, and a policy that approves or denies the whole call is the
  wrong shape. Either the store filters by owner (which pushes policy into the
  store — see the caution in [03](03-postgres-entity-store.md)) or the
  `SecurityManager` gains a way to express a filter.
- Ownership alone will not cover shared agents, teams, or an agent that acts
  on behalf of several users. It does not have to — but the model should not
  make those impossible to add later.
- Deletion and ownership interact: what happens to an agent when its owner is
  removed?

## Done when

- Entities record their owner and it is populated from the authenticated
  caller.
- Reads and writes of entities the caller does not own are denied, including
  in list results.

## History

- **2026-09-10** — Issue created from `TODO.md`.
