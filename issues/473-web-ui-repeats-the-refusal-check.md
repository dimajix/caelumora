# 473 — Every dialog of the web UI repeats which errors it shows itself

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** User interaction
**Summary:** Whether a dialog shows what the server said or reports the error is decided by the same condition copied into eighteen files.

## The problem

`error instanceof ApiRequestError && error.status < 500 && error.status !== 401` decides, in every
dialog of `caelumora-web-ui`, that the server's message is the user's to read in the dialog - a name
taken, a scope not allowed -, and anything else is reported (`reportError`). It stands in eighteen
files, three of which issue [467](467-projects-hold-what-outlasts-a-session.md) added
(`NewProjectDialog`, `SessionProjectDialog`, `ProjectPage`).

## Why it matters

What counts as the request's fault is one rule of the client; written out eighteen times, a change
of it - another status, a code to tell apart - is made in some of them and forgotten in others.

## Considerations

- **A. One function in `errors.ts`**, e.g. `refusalOf(error): string | null`, the message to show in
  the dialog or `null` for one to report; every dialog uses it.
- **B. A hook** that holds the problem's state as well (`useRequestProblem`), since every dialog
  keeps a `problem` beside it.

Recommendation: A, and B only where it makes the dialogs shorter.

## Decision

Not the condition alone (A): what stood eighteen times is the whole branch of the `catch` - show
the server's message in the dialog, or report the error -, and a `refusalOf` would leave it at
every dialog. `reportUnlessRefused(action, error, show)` in `errors.ts`, beside `reportError` and
taking its arguments in the same order, holds the condition and both outcomes; every dialog's
`catch` is one call of it. No hook (B): it would only take the `useState` and the `setProblem(null)`
off the dialogs, at the price of a concept with three parts.

## Done when

- The condition stands in one place, and every dialog asks it there.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 4b of issue 467.
- **2026-10-28** — The maintainer decided for a function that holds the whole branch of the
  `catch`. Done: `reportUnlessRefused` in `errors.ts`; the eighteen dialogs call it.
