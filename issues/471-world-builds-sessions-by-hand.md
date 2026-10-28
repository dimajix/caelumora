# 471 — The test world stores its sessions by hand instead of making them as the runtime does

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `World::session_of`, which most tests of the security suite start from, stores a session, its groups, its own project and its channel itself, and so has to be changed whenever making a session gains a step.

## The problem

`World::session_of` (`caelumora-runtime/src/testing/mod.rs:424`) builds a session and its channel
as literals, stores them in the runtime's name, makes the groups of their participants and - since
issue [467](467-projects-hold-what-outlasts-a-session.md) - the session's own project
(`session::make_own`), writes the channel's state with its founders, and only then opens the
session. `SessionManager::create` (test support) and `establish` do the same as the runtime does it.

## Why it matters

- Every step making a session gains has to be repeated here, or the suite tests a session the
  runtime never makes: 467 had to add the own project by hand.
- What a test asserts about the questions asked (`World::asked`) leaves out what making the session
  asks, which may be right for the tests, but is decided here by accident.

## Considerations

- **A. Go through `SessionManager::create`**, in the runtime's name, with the founders given as the
  channel's participants; the session's id and its channel come back from the loop. The lines of
  `asked` the tests compare are taken after it, as now.
- **B. Keep it, and test that it builds what `create` builds** - a test comparing both.

Recommendation: A.

## Done when

- `World::session_of` makes its session through the runtime; no test of the suite builds a session's
  groups or project by hand.

## History

- **2026-10-28** — Opened at the maintainer's request from the findings of issue 467.
