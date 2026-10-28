# 479 — A rename the server refuses is reported in the banner, not where it is typed

**Status:** open
**Type:** feature
**Priority:** low
**Area:** User interaction
**Summary:** When the server refuses a new name for a project, team, agent or session - a name taken, say - the refusal appears in the error banner, away from the field where it can be corrected.

## The problem

The names of a project (`ProjectPage.tsx:85`, also its description), a team (`GroupPage.tsx:50`),
an agent (`AgentPage.tsx:48`) and a session (`Sidebar.tsx:348`) are edited in place. Whatever the
server answers, the edit's `catch` hands it to `reportError`, so a name taken shows up in the
banner at the top. Every dialog, and the form "keep the project" on the same `ProjectPage`, shows
the same refusal next to its field, through `reportUnlessRefused` (issue
[473](473-web-ui-repeats-the-refusal-check.md)).

## Why it matters

[WEB-UI.md](../docs/WEB-UI.md) states that what the server refuses is shown where it can be
corrected, and names only dialogs; the edits in place follow another rule without a reason. A
refusal in the banner is read away from the field, and the field has already closed.

## Considerations

- **A. The edits in place keep a `problem` of their own**, call `reportUnlessRefused` in their
  `catch`, stay open on a refusal and show it below the field.
- **B. The rule is narrowed to dialogs** in WEB-UI.md, with the reason why an edit in place reports
  instead.

## Done when

- A refused rename is shown where the name is typed, or the document says why it is not.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of issue 473.
