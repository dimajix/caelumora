# 338 — An artifact is published into its session's scope and goes with the session

**Status:** rejected
**Type:** feature
**Priority:** high
**Area:** Context and knowledge
**Summary:** An artifact is published into its session's scope and is deleted with the session unless its owner keeps it elsewhere, though it is meant to outlive the conversation.

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

## Proposal

As it stood when the issue was deferred (2026-10-13); not decided.

- **A**, worded generally: an artifact lies in its owner's scope. Today that is always a user;
  should a team own things one day (issue [340](340-teams-as-owners.md)), it lies in the team's,
  with nothing changed about its lifetime or how it is found by name. The `ArtifactManager` is no
  `ScopeMember` any longer; `Artifact::validate` accepts the owner's scope alone.
- **Another's artifact is named by its id.** `publish_artifact` takes an optional parameter
  `artifact`: with it, the version is added to that artifact (`WriteArtifact`); without it, the
  artifact of that name in the publisher's scope gains one or is made. `artifact` and `name`
  together are refused. The artifact is fetched in the caller's name (`GetArtifact`), so that one
  who cannot reach it is told `NotFound` and learns nothing of the id. Weighed against it: finding
  by name among the artifacts published in the channel - a name is not unique there, and who meant
  their own would add to another's unnoticed -, and a name qualified by its owner
  (`alice/report.csv`) - a model does not know user names reliably. The cost: an agent learns
  another's id only from a person; a tool that lists artifacts would be an issue of its own.
- **`keep` goes.** The global scope grants nobody anything for an artifact - its lists do -, and
  nothing finds an artifact there by name; and an artifact moved there would be followed by a second
  one of the same name in its owner's scope at the next publication. So an artifact always lies in
  its owner's scope, and `ArtifactManager::keep`, `ArtifactError::InvalidScope`,
  `ArtifactUpdateEvent::ScopeChanged`, `PUT /artifacts/{id}/scope` and the buttons to keep in the
  web UI (`ArtifactPage.tsx`, `ToolCallView.tsx`) are removed - a change of the REST API, with the
  web UI changed along.
- **The tests of issue 320** take a contributor's agent as what is left in a session; the security
  suite gains adding to another's artifact by its id, `NotFound` for one who cannot reach it
  included.
- **Stored data to delete:** artifacts lying in a session's scope.

## History

- **2026-10-13** — Opened at the maintainer's request, during issue
  [316](316-store-checks-artifact-without-versions.md): artifacts are not to be bound to a channel
  or a session, and that they are deleted with the session is a bug.
- **2026-10-13** — Discussed, and deferred by the maintainer; the proposal as it stood is recorded
  above, under *Proposal*. Whether teams should own things - which would make adding to another's
  artifact rarer and give an artifact of a team a place of its own - went into the memo
  [340](340-teams-as-owners.md).
- **2026-10-28** — Rejected in favour of issue [467](467-projects-hold-what-outlasts-a-session.md),
  which the maintainer opened while taking this one up: an artifact lies in the project its
  session's results go to, and goes with the session only where that project is the session's own.
  The proposal above - an artifact in its owner's scope, a version added by id - is not done;
  keeping an artifact by itself is gone, and lifting the session's own project takes its place. The
  bug of this issue is a rule now.
