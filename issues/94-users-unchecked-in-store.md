# 94 — The store does not check access to users

**Status:** open
**Area:** Identity & access

## The problem

`store_impl/memory.rs:293-332` and `json.rs:535-585` carry
`// Placeholder for security check` on every user method, although
`ActionType::GetUser` exists and `SecurityAttributes::owned_by(User)`
(`user.rs:57`) is there for exactly this. Every other kind goes through
`store_impl/access.rs`.

## Why it matters

A user record is readable and writable by anyone with a store handle. Issue
[25](25-file-user-backend.md) leaves open who owns the record - provider or
store - but not whether access to it is checked.

## Considerations

`User: Secured` with `owned_by(ActorId::User(id))` and the same `Access`
calls as the other kinds; eight methods, mechanical. The `SecurityManager`
gate for *lookups* is a different question (25 decided that lookups are not
gated like secrets) - this issue is about the store's own check.

## Done when

No store method carries a placeholder comment; user access goes through
`Access`.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.2.4).
