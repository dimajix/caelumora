# 483 — The sidebar edits a session's name in place by hand

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** User interaction
**Summary:** The field in which the sidebar renames a session repeats what `EditInPlace` does, without what that has since learned: Escape is followed by a save, and a name that differs only by spaces is sent.

## The problem

Issue [479](479-a-rename-refused-goes-to-the-banner.md) made every text a page edits in place an
`EditInPlace`, but left out the session's name in the sidebar (`Sidebar.tsx`, `handleNameSave`,
`handleNameKeyDown`): its field stands in a row of the list, inside a button, so that its refusal
goes to the banner. Its field is written out by hand, and it lacks what `EditInPlace` does:

- Escape removes the field, and the blur that may follow saves the text after all.
- It compares the untrimmed text with the stored name, so that "Name " asks the server to rename
  the session to the name it has.
- Nothing keeps a second save away while one is under way.

## Why it matters

Two ways of editing a text in place, one with the faults the other was rid of; a fix to either is
not made in the other.

## Considerations

- **A. A hook for what a text edited in place does**, e.g. `useEditInPlace`: the phase - editing,
  saving, closed -, trimming, skipping an unchanged text, and where a refusal goes. `EditInPlace`
  shows it under its field, the sidebar hands it to the banner.
- **B. Align the sidebar by hand** - an Escape that saves nothing, a trimmed comparison -, keeping
  the two copies.

## Done when

- Editing a session's name in the sidebar behaves as `EditInPlace` does, but for where a refusal is
  shown, and its rules stand in one place.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 3 of issue 479.
