# 338 — An artifact is published into its session's scope and goes with the session

**Status:** open
**Area:** Context and knowledge

## The problem

`ArtifactManager::publish` (`caelumora-runtime/src/artifact/mod.rs`) makes an artifact in the
scope of the session it is published in (`Scope::SessionScope`), and finds an artifact to add a
version to by its name there. The `ArtifactManager` is a member of that scope (`ScopeMember`):
deleting, restoring and purging the session does the same to every artifact still lying in it -
its content included -, unless its owner moved it into their own scope or the global one before
(`ArtifactManager::keep`, `PUT /artifacts/{id}/scope`). This was decided in issue
[271](271-publishing-artifacts.md) (open question 2: published into the session's scope, kept by
the user as an act of theirs).

Since issue [320](320-scoped-deletion-in-the-runtimes-name.md), what lies in the scope goes with it
in the name of whoever deletes it: an artifact another participant published in the session is not
theirs to delete, and the session cannot be deleted until its publisher keeps it elsewhere
(`SessionError::ScopeNotEmpty`).

## Why it matters

- An artifact is a result meant to outlive the conversation it came from - that is what sets it
  apart from a workspace, which may have been made for one session only (issue 271, *Not the
  workspace*). Bound to the session, it shares exactly the lifetime it is meant to escape: whoever
  does not think of keeping it loses it with the session.
- An artifact is not bound to a channel or a session: it is published *in* one, which its version
  records (`session_id`, `channel_id`) and which decides who reads that version (issue 23), but it
  does not *lie* there.
- A session one participant published an artifact in cannot be deleted by another until the
  publisher acts.

## Considerations

- **A. Published into its publisher's own scope.** The artifact lies in the scope of the user it
  is published for (`ctx.principal_id`), and a name is found there: publishing under the same name
  again - from any session - adds a version to it. `keep` becomes moving it between its owner's
  scope and the global one. Deleting a session leaves artifacts alone; the `ArtifactManager` is no
  member of a session's or channel's scope any longer. What changes besides:
  - Adding a version to another's artifact (`WriteArtifact`, issue 23) works today because both
    find it by name in the session's scope. In the publisher's scope, `artifact_tool` would find
    their own; adding to another's needs that artifact named - by its id, or its owner's scope with
    its name.
  - The tests of issue 320 use another's artifact as what is left in a session; they take another
    kind there - a contributor's agent.
- **B. In the session's scope, moved to its owner's when the session goes.** Keeps finding by name
  within the session, but gives the session's scope a meaning for artifacts it has for nothing else,
  needs a name free in the owner's scope at that moment, and makes restoring the session ask
  whether to move them back.
- **C. In no scope.** Names not unique anywhere, so a name cannot find the artifact to add a
  version to; and the global scope (shared with everyone it is shared with) would have no place.

Recommendation: A - the scope says where an artifact lives, and that is with its owner. It also
takes a `ScopeMember` out of what issue [328](328-artifacts-in-a-crate-of-their-own.md) moves into
a plugin. Stored data to delete: artifacts lying in a session's scope.

## Done when

- Deleting, restoring or purging a session does nothing to the artifacts published in it.
- It is decided how a version is added to another's artifact, and `artifact_tool` does so.
- `docs/` and the history of issue 271 say where an artifact lies.

## History

- **2026-10-13** — Opened at the maintainer's request, during issue
  [316](316-store-checks-artifact-without-versions.md): artifacts are not to be bound to a channel
  or a session, and that they are deleted with the session is a bug.
