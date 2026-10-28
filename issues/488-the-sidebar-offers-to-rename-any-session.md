# 488 — The sidebar offers to rename a session whoever may change it

**Status:** done
**Type:** bug
**Priority:** low
**Area:** User interaction
**Summary:** A session's menu in the sidebar offers "Rename" also to who may not change the session, who types a name and only then sees it refused; "Language" and "Time zone" beside it are offered only under `can_change`.

## The problem

The menu behind a session's three dots (`Sidebar.tsx`, the session's `ActionMenu`) lists "Rename"
for everyone. A user who may not change the session opens the field, types a name, and the server
refuses it under the field (issue [483](483-the-sidebar-edits-a-session-name-by-hand.md)). The
entries "Language" and "Time zone" of the same menu are listed only where `session.can_change`
holds, and `AgentPage` offers "Rename" only under `agent.can_change`. `can_change` of a session is
documented as "whether you may change it - rename it, change its privacy, set its approval rules".

## Why it matters

[WEB-UI.md](../docs/WEB-UI.md) says of menus: "It lists only what may be done". The session's menu
lists what may not, and the user learns it only after typing. Found in the review of issue 483.

## Considerations

- **A. List "Rename" under `can_change`**, beside "Language" and "Time zone".
- "Delete" in the same menu is listed for everyone too, but it asks for another permission - to
  delete the session - of which the session's answer says nothing; it is not part of this issue.

## Done when

- The session's menu lists "Rename" only to who may change the session.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of issue 483.
- **2026-10-28** — Done as A: the session's menu lists "Rename" under `can_change`, at its place
  before the other entries. The other entries listed for everyone - "Description", "Privacy",
  "Approval rules", "Workspace", "Project" - open what everyone reads and only who may change the
  session changes; "Rename" changes alone, as "Language" and "Time zone" do. A refusal of a rename
  still goes to the banner, not under the field, until issue 483 is done.
