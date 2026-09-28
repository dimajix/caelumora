# 87 — Stored entities carry a `SecurityContext`

**Status:** open
**Area:** Identity & access

## The problem

`Agent::security_context` (`caelumora-model/src/agent.rs:147`) and
`User::security` (`user.rs:16`) are fields of stored documents. Both actors
overwrite them at start (`actor_impl/agent.rs:84-85`, `user.rs:46-47`,
setting `session_id`/`channel_id`), so the controller then holds a
channel-specific copy of a stored object. A context is something an actor
*has* in a place, not something a document *is*.

## Why it matters

A stored context is stale by definition, and the overwrite hides that the
value is derived. Issue [10](10-user-authentication.md) wants the fabricated
contexts to become one greppable place; a context inside every entity is
the opposite.

## Considerations

Remove the fields; the actor builds
`SecurityContext::new(ActorId::…, session_id, channel_id)` from what
`ActorRuntime` already knows. Two runtime places and the test fixtures.
`User::roles` and `User::email` have no reader outside the model either,
but issue [23](23-acls-in-security-attributes.md) keeps `roles` open - leave
them.

## Done when

No stored entity has a `SecurityContext` field; actors derive theirs.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.1.5).
