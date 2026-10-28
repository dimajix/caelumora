# 479 — A rename the server refuses is reported in the banner, not where it is typed

**Status:** done
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

## Decision

A, for every text edited in place, in three steps; the session's name in the sidebar keeps the
banner (B for it alone).

1.  **The width of comments is checked in TypeScript too.** Taken in at the maintainer's request:
    `scripts/line-length.sh` checks the comments of `caelumora-web-ui/src`, the pre-commit hook
    hands it those files, and the comments longer than 100 characters are rewrapped.
2.  **`EditInPlace`**, without a change of behaviour: the issue names four places, but a text is
    edited in place nine times on six pages - the names and descriptions of projects, workspaces and
    skills, the names of teams and agents, the description of an artifact -, each field written out
    by hand. One component takes their place.
3.  **A refusal stays where it is typed.** `EditInPlace` stays open until the server answered, and
    shows a refusal under the field through `reportUnlessRefused`, the text kept; Escape gives up,
    and leaving the field saves again. `Refusal` gets `on: 'dialog' | 'page'`, `'dialog'` by
    default, for a refusal on a page that follows the theme - `EditInPlace`'s, and that of the form
    "keep the project", which loses its own line. The sidebar's field for a session's name stands
    in a row of a list, inside a button, with no room for a line below; there a refusal stays in the
    banner, and WEB-UI.md says why.

## Done when

- A refused rename is shown where the name is typed, or the document says why it is not.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of issue 473.
- **2026-10-28** — Step 1 done: `line-length.sh` checks the comments of the web UI's TypeScript,
  and the 147 lines longer than 100 characters are rewrapped.
- **2026-10-28** — Step 2 done: the nine fields of six pages are `EditInPlace`, which trims the
  text, stores nothing unchanged and reports a failure; each page says only whether an empty text
  is stored, and how.
- **2026-10-28** — Step 3 done: `EditInPlace` stays open until the server answered and shows a
  refusal under the field; what else fails is reported, and the field stays open too, so that the
  text is kept. A blur while a save is under way, or as the field closes, saves nothing - also after
  Escape, which saved the text before. An answer that comes once the field is closed - given up, or
  gone with its page - closes nothing, and its refusal goes to the banner. `Refusal` has `on`,
  `'dialog'` or `'page'`; the form "keep the project" uses it, in red-600, or red-400 in the dark
  theme, as the login shows its errors.
- **2026-10-28** — The session's name in the sidebar no longer keeps the banner: issue
  [483](483-the-sidebar-edits-a-session-name-by-hand.md) put its field in place of the row's button,
  where it is an `EditInPlace` and shows a refusal under it.
