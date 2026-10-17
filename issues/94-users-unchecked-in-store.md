# 94 — The store does not check access to users

**Status:** done
**Area:** Identity and access management
**Epic:** [74](74-refactoring-epic.md)
**Summary:** The store checks access for every entity kind except users, where a placeholder comment stands in for the check.

## The problem

`store_impl/memory.rs:293-332` and `json.rs:535-585` carry `// Placeholder for security check` on
every user method, although `ActionType::GetUser` exists and `SecurityAttributes::owned_by(User)`
(`user.rs:57`) is there for exactly this. Every other kind goes through `store_impl/access.rs`.

## Why it matters

A user record is readable and writable by anyone with a store handle. Issue
[25](25-file-user-backend.md) leaves open who owns the record - provider or store - but not whether
access to it is checked.

## Considerations

`User: Secured` with `owned_by(ActorId::User(id))` and the same `Access` calls as the other kinds;
eight methods, mechanical. The `SecurityManager` gate for *lookups* is a different question (25
decided that lookups are not gated like secrets) - this issue is about the store's own check.

## Done when

No store method carries a placeholder comment; user access goes through `Access`.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 3.2.4).
- **2026-09-29** — Done, as agreed with the maintainer - but not as the considerations propose.
  Owning their own record would let a user change their own `roles`, and would hide every other user
  from them, so that a session with a second person could not be made (`instantiate` reads each user
  participant as the session's owner). A user record belongs to the system instead: `User: Secured`
  returns system-owned attributes from a constant, derived rather than stored so that no record can
  say otherwise. Under the ownership policy everybody reads and lists users - a directory, in line
  with issue 25's ungated lookups - and only the system creates, changes or deletes them.
  `ActionType::{ListUsers, CreateUser, UpdateUser, DeleteUser}` are new; both stores route users
  through the same `Access`-checked helpers as every other kind (`store_impl::access::USERS`), and
  `find_user_by_name` checks like a list. The TUI now records its terminal user as the system, like
  the REST setup and `AcceptAnyUserProvider` already did. Left as it is: `UserCreated`/
  `UserUpdated` go to the user alone although the record is readable by all. Found on the way:
  sessions and channels embed whole user records - issue [120](120-participant-copies.md).
