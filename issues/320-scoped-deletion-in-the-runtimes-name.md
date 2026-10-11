# 320 — What lies in a scope is deleted and restored with it in the runtime's name

**Status:** done
**Area:** Identity and access management

## The problem

What lies in a session's or channel's scope is deleted, restored and purged with it (issue 178,
`caelumora-runtime/src/scoped.rs`). All of it is done with `SecurityContext::system()`, which the
policy allows everything: whoever may delete the session deletes what lies in its scope, "and is
asked nothing more" (`docs/SECURITY.md`).

With the system's context the store asks nothing at all - not the one question the rule is about,
whether the caller may delete what lies in the scope, but none of the others either. The rule
itself is decided by the runtime, by passing that context, and no policy can decide it otherwise.

What lies in a session's scope is not always the session owner's. An artifact is published into
the session's scope by whoever publishes it, and stays there until its owner keeps it elsewhere
(issue 271); an agent of a contributor lies there too. Its owner is not asked when it goes with the
session, nor when it comes back with it.

The store also marks what it deletes as deleted by the caller of the context (`Removal::by`):
everything deleted with a session is marked as deleted by the system, not by whoever deleted the
session, and the views show it so.

## Why it matters

- One person deletes what belongs to another without the policy being asked.
- A restore in the system's name brings back what its owner could not have restored.
- The record of a deletion names the system instead of the person who caused it.

## Considerations

- **Listing stays in the runtime's name.** What lies in the scope is listed with the system's
  context, so that everything in it is found, whatever the caller may read. Nothing of what is
  listed reaches the caller.
- **Deleting in the caller's name, holder kept if something remains.** Everything in the scope the
  caller may delete is deleted in their name, so that the store asks and marks it as theirs. If
  something remains - another's artifact, a contributor's agent -, the session or channel is not
  deleted, and the caller is told by an error of its own that says what remains (kinds and counts,
  no names: what lies there is not necessarily the caller's to read). What remains is then clear:
  its owners keep it elsewhere or delete it, and the holder is deleted on the next attempt.
  This leaves the holder alive with part of its contents deleted until then; that was weighed
  against asking everything first and changing nothing on a refusal, and chosen because it shows
  plainly what is left to do.
- **Restoring in the caller's name, holder restored regardless.** Everything the caller may
  restore is restored in their name; what they may not stays deleted, and its owner restores it
  through its manager once the holder is back. The holder is restored all the same: a holder
  missing part of its contents is a state that also arises by hand, while a deleted holder with
  live contents is not (`Scope::admits`). Whether the restore then reports what stayed deleted is
  open; an element whose name was taken meanwhile is only logged today (issue 250), and the
  restore returns what it restored (issue 318).
- **Purging** is the system's or an administrator's anyway (`Purge`); it can take the caller's
  context as well, which changes nothing of who is asked.
- **Where the context is built.** Since issue
  [319](319-scoped-artifact-events-miss-version-readers.md) every kind with an owner deletes and
  restores what lies in a scope through its manager (`…_lying_in`, called by `ScopedEntities`):
  the caller's context is passed there, and to the store for personalities and session templates.

## Done when

- What lies in a scope is listed in the runtime's name and deleted and restored in the caller's,
  and the store marks it as deleted by the caller.
- Deleting a session or channel whose scope holds something the caller may not delete deletes
  what they may, keeps the session or channel, and fails with an error of its own that the REST
  server maps to a status of its own.
- Restoring a session or channel restores it and what the caller may restore of its scope.
- Tests in the security suite show both, with an artifact another participant published in the
  session.
- `docs/SECURITY.md` says what the rule now is.

## History

- **2026-10-11** — Opened at the maintainer's request during issue
  [319](319-scoped-artifact-events-miss-version-readers.md). Decided with the maintainer: listing
  in the runtime's name, deleting and restoring in the caller's; on deleting, what the caller may
  delete is deleted and the holder kept if something remains, with an error of its own; on
  restoring, the holder is restored even if not all of its scope can be. Whether a partial restore
  reports anything is not decided yet.
- **2026-10-11** — Done. What lies in a scope is listed in the runtime's name and deleted and
  restored in the caller's, through `ScopedEntities` and the managers' `…_lying_in`; the store
  asks each item and marks it as deleted by the caller. A refusal counts as left on deleting and
  as staying deleted on restoring. Deleting what is left fails with `SessionError::ScopeNotEmpty`
  (kinds and counts), which the REST server answers with `409 scope_not_empty`; the groups of the
  participants are deleted last, and only once nothing else is left, since the holder's access
  depends on them. Decided with the maintainer: a partial restore reports nothing - what stays
  deleted is logged, as a taken name is. Since `restore_lying_in` brings back what was deleted at
  or after the holder's mark (issue 178), the holder is now marked before its scope is deleted, and
  the mark taken away again in the runtime's name if something is left (option A, chosen over
  recording in `Removal` what something was deleted with, which would change the store's API for
  every kind): a deletion that succeeds comes back whole; what an attempt that failed deleted stays
  deleted on a later restore, like something deleted by hand, and is restored by itself. A channel
  put back that way is listed last in its session. Tests in the security suite with an artifact
  another participant published in the session; `docs/SECURITY.md`, `CHANNELS.md`,
  `REST-SERVER.md` and `PERMISSION-CHECKS.md` say what the rule now is.
