# 476 — Making a session in a project is a sequence of the REST handler, with its roll-back

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Sessions and channels
**Summary:** A session asked for in a shared project is made, put into the project and, if that fails, rolled back by the REST handler, so that every other client would have to repeat the sequence.

## The problem

Decided with the maintainer during step 4a of issue
[467](467-projects-hold-what-outlasts-a-session.md): a session is made in its own project and put
into a shared one by `SessionLoop::update_project`, so that `create_from_template` keeps its
parameters and there is one way into a project. The handler `create_session`
(`caelumora-rest-server/src/routes/sessions.rs:35`) does it: `create_from_template`, then
`update_project`, and on a failure `SessionManager::roll_back_creation`, which was made public for
it.

## Why it matters

The REST server is to map HTTP to the runtime, nothing more (ARCHITECTURE.md); a sequence with a
roll-back is the runtime's. With one caller it does no harm; a second - the TUI making a session in
a project - would copy it.

## Considerations

- **A. One step of the runtime**, e.g. `SessionLoop::enter_project_or_roll_back` or a
  `SessionManager` method taking the session made and the project, which keeps `roll_back_creation`
  private again.
- **B. The project as a part of what a request asks for**, a value `create_from_template` takes
  beside name, language, time zone and workspace - the bundling of parameters weighed and set aside
  in 467.
- **C. Leave it** until a second caller comes.

Recommendation: C, then A.

## Done when

- Making a session in a project is one call of the runtime, and `roll_back_creation` is private.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 4a of issue 467.
