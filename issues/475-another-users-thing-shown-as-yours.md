# 475 — The web UI calls another user's workspace, team or project "yours"

**Status:** done
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
- **2026-10-28** — Decided with the maintainer: A, the name only for `scope: user`, and a
  restructuring first. Of what lies in a user's scope, that user is its owner (`Stored::validate`
  of workspaces, projects and groups), so a user's scope names its user as a session's names its
  session; the owner's name is given there alone, which a global thing does not need. Step 1,
  without a change of behaviour: the six responses that repeated where something lies - workspace,
  project, agent, personality, group, skill - carry it as one `ScopeResponse` (`ScopeResponse::of`,
  in place of `ScopeKind::of` and `ScopeKind::placed`), the JSON unchanged; named so, not
  `Place…`, since `confidentiality::Place` is another concept. Step 2: `ScopeResponse` names the
  user of a user's scope and their name, the runtime reads the name for the snapshots
  (`Views::name_of`), SECURITY.md says that whoever reads something of another's scope learns
  their name, and the web UI says "yours" or whose it is - also in the lists, and on a project's
  page again -, its six response types sharing one interface for the place as the Rust ones do.
- **2026-10-28** — Step 2: the snapshots carry where something lies as a `ScopeSnapshot` - its
  scope and, for a user's, that user's name, read by `Views::scope_snapshot` -, and `ScopeResponse`
  answers a user's scope with `user_id` and `user_name`. The web UI's six response types share one
  `ScopeResponse` interface; the pages, lists and dialogs that say where something lies are given
  the viewer's id (as `AgentPage` was), and `scopeLabel` says "yours", "of <name>" or "another
  user's"; a template's workspace, whose `user` scope is that of whoever uses it, is labelled by its
  kind alone (`kindLabel`). A project's page shows where it lies again.
