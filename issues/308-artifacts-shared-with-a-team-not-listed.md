# 308 — An artifact shared with a team is not listed among its members' artifacts

**Status:** open
**Area:** Identity and access management

## The problem

Since issue [286](286-groups-as-subjects.md) an artifact can be shared with a team: an entry
`Subject::Group(team)` in its access list, or in a version's. A member of the team may read it -
`GetArtifact` grants it, and `Views::artifact(id)` reaches it. But `Views::artifacts`, the list
`GET /artifacts` answers and the web UI's sidebar shows, does not list it.

The list asks the store, in the runtime's name, for the artifacts that name the viewer: those they
own, and those whose list or a version's list names them as `Actor` or one of their roles as
`Role` (`ArtifactField::Subject`, issue 293). Each is then asked of the policy. A `Group` entry is
never among the subjects asked for, so an artifact shared with a team only is never a candidate -
however the policy would answer.

Workspaces do not have the problem: `Views::workspaces` lets the store list every workspace and
filters each by `GetWorkspace`, which resolves the group.

## Why it matters

- Sharing an artifact with a team is what teams are for; a member who does not find it in their
  list has to be sent its link.
- The list and the reach answer differently for the same artifact, which is the kind of
  inconsistency issue 304 is about.

## Considerations

- **Ask for the viewer's teams as well.** Before the store is asked, the teams the viewer belongs
  to are looked up and named as subjects besides their id and roles. A user belongs to few teams,
  unlike to sessions and channels; but it is the reverse lookup - "which groups is this actor
  in" - that issue 304 decided the policy never makes. Here it is the view that makes it, to list,
  not the policy to decide; the store needs a condition on a group's members (a field of the
  `Group` kind) to answer it without a scan.
- **Filter like workspaces.** Let the store list every artifact and filter each by `GetArtifact` and
  each version by its own list. Simple and consistent with workspaces, but a scan of all artifacts
  for every list, which issue 293 moved away from.
- **List teams' artifacts on the team's page.** Keep `Views::artifacts` as it is and show what is
  shared with a team where the team is shown. Leaves the inconsistency of the list in place.

The first is the likely answer: it keeps the list a query and names the teams once per list.

## Done when

- An artifact shared with a team - as a whole or by a version - is listed among the artifacts of
  every member of the team, and no longer among those of who left it.
- The security suite covers it, as G1 of issue 286 covers a workspace.

## History

- **2026-10-11** — Opened at the maintainer's request, found during step 2a of issue
  [304](304-review-of-the-permission-concept.md): `Views::artifacts` names the viewer and their
  roles to the store, never a group.
