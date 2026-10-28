# 475 — The web UI calls another user's workspace, team or project "yours"

**Status:** open
**Type:** bug
**Priority:** low
**Area:** User interaction
**Summary:** What lies in a user's scope is labelled "yours" in the web UI, also where it is another user's, shared with the viewer, since the responses do not name its owner.

## The problem

`scopeLabel` (`caelumora-web-ui/src/scope.ts`) answers "yours" for every `scope: user`. A workspace
shared with the viewer (`WorkspaceResponse`), a team the viewer belongs to and a project they work
on lie in their owner's scope, so another user's is shown as the viewer's own - on its page ("Name
space: yours") and wherever it is chosen. `WorkspaceResponse` and `ProjectResponse`
(`caelumora-rest-api`) carry no owner; `GroupResponse` does. Issue
[467](467-projects-hold-what-outlasts-a-session.md) shows no scope for a project of a user's scope
for this reason (`placeOf`, `optionLabel` in `caelumora-web-ui/src/project.ts`).

## Why it matters

A user deciding what to attach, share or publish into reads "yours" for what belongs to someone
else, whose owner can change or delete it.

## Considerations

- **A. Name the owner in the responses** (`owner: ActorId`, and its name where the server knows it),
  and let `scopeLabel` say "yours" for the viewer's own and "of <owner>" for another's.
- **B. Answer whether it is the viewer's** (`is_own: bool`), which is all the label needs, and no
  name.

Recommendation: A, as `GroupResponse` already does it; the project's page then shows its scope
again.

## Done when

- Nothing another user owns is labelled "yours"; the web UI names whose it is.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 4b of issue 467.
