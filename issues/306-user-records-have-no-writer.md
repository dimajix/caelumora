# 306 — A user's record is written by whichever provider authenticates, and nobody hears of a change

**Status:** open
**Area:** Identity and access management

## The problem

A `User` record belongs to the system (issue [94](94-users-unchecked-in-store.md)): its roles decide
what the user is cleared for and which access lists name them. As of 2026-10-11 it is written by
the user provider that authenticates the user - `AcceptAnyUserProvider::authenticate` creates it,
and at every login stores the roles the configuration gives, if they differ - directly into the
entity store, with no event.

So nothing in the runtime learns that a user's roles changed:

- `UserUpdateEvent` knows only `NameChanged`, and nothing in production publishes it.
- The `UserController`, the live user of an instance, holds the `User` with its roles but follows
  only that event, so its roles are those it was made with.
- Therefore every place that decides by roles reads them from the store instead, past the
  controller: `channel::user_roles`, `Member::clearances`, `Views::roles`, `SkillManager`,
  `WorkspaceManager`, `StoreMemberships::roles_of` (issue [181](181-where-roles-decide.md), history
  of 2026-10-02).
- Each future provider - a file (issue [25](25-file-user-backend.md)), a database (26), a directory
  (27) - would have to write the record, and announce its change, on its own.

## Why it matters

- Issue [304](304-review-of-the-permission-concept.md), step 2, carries a subject's roles in its
  `SecurityContext`, computed once where the context is built. Read from the store, that is a store
  read for every request; held by the `UserController`, it is free - but only if the controller
  follows changes.
- Several instances (issue [122](122-several-instances.md)) are kept in step through the global
  events; a change nobody announces reaches no other instance.
- A role taken away takes effect only where it is read again; nobody can say when that is.

## Considerations

Decided with the maintainer on 2026-10-11:

- **The `UserManager` is the one writer of user records.** A provider says who the user is by its
  source - id, name, roles - and nothing else; the `UserManager` compares that with the record
  stored, stores it in the system's name where it differs, and announces it. `AcceptAnyUserProvider`
  only reads. A provider whose source is the store itself, as `accept_any`'s is, answers the stored
  record with the roles its configuration gives.
- **An event for roles**: `UserUpdateEvent::RolesChanged`, published with the user's attributes
  where they changed; a new record is announced as `UserCreated`, which exists.
- **The `UserController` follows it**, so that the roles it holds are current - in every instance.
- **When it is compared**: at a login, and when the REST server asks the `UserManager` about the
  user on renewing a token, so that a role taken away takes effect within half a token's lifetime,
  not at the next login. For `accept_any` that matters once its configuration can change without a
  restart.
- **What it costs**: `AppConfig::create_user_manager` takes an `EventPublisher`; the REST server's
  `main` builds the user manager after the event bus. No stored data changes.
- **Not part of it**: reading the roles from the controller where permissions are decided - that is
  step 2 of issue 304, which builds on this. Nothing here changes what any check decides.

## Done when

- User records are written by the `UserManager` alone, and every change of them is announced.
- The `UserController` holds the roles as they are stored, after a login or a renewal changed them.
- Tests cover a role given and taken at a login and at a renewal, and the controller following it.

## History

- **2026-10-11** — Opened at the maintainer's request, before step 2 of issue 304: the roles in a
  `SecurityContext` are to come from the `UserController`, which does not follow role changes
  today. The `UserManager` as the one writer was decided with it.
