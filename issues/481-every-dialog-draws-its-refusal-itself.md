# 481 — Every dialog of the web UI draws the server's refusal itself

**Status:** done
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

## Decision

A, drawn for a dialog: `components/Refusal.tsx`, as the fields dialogs share (`LanguageField`,
`ClassificationFields`) are. The colour of `ProjectPage` was not without a reason: a dialog is dark
whatever the theme, the form "keep the project" lies on the page and follows the theme, where
`text-red-400` is hardly read on white; the theme is the system's (a media query), so a component
cannot tell which it stands on. That form keeps its own line, saying why. A variant for the page
waits for a second user - issue [479](479-a-rename-refused-goes-to-the-banner.md) would bring
four -, and one class for both, `text-red-600 dark:text-red-400`, would be read worse in a dialog
in the light theme. With it, each dialog's state `problem` is `refusal`: it holds what
`reportUnlessRefused` calls one.

## Done when

- The look of a refusal in a dialog is written in one place.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of issue 473.
- **2026-10-28** — Done: `Refusal` draws the refusal in the seventeen dialogs; `ProjectPage`'s form
  keeps its own, for the page's theme. The state is `refusal` in all eighteen.
- **2026-10-28** — Issue [479](479-a-rename-refused-goes-to-the-banner.md) gave `Refusal` a look
  for a page, which the form "keep the project" uses instead of a line of its own.
