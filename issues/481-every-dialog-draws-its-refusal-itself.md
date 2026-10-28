# 481 — Every dialog of the web UI draws the server's refusal itself

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** User interaction
**Summary:** How a refusal is shown in a dialog - an alert line under the fields - is written out by hand in eighteen files, and one of them already draws it in another colour.

## The problem

Every dialog that shows a refusal (issue [473](473-web-ui-repeats-the-refusal-check.md)) renders
it as `{problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}`, eighteen
times; `ProjectPage.tsx:313` writes `text-red-500`, apparently without a reason.

## Why it matters

How a refusal looks is one decision of the client; copied into every dialog, a change of it - its
colour, an icon, how a screen reader announces it - is made in some and missed in others, as the
colour already was.

## Considerations

- **A. One small component**, e.g. `<Refusal message={problem} />`, which draws nothing for `null`;
  every dialog uses it.

## Done when

- The look of a refusal in a dialog is written in one place.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of issue 473.
