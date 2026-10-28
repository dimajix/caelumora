# 485 — A session's own project is derived from its owner in several places

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Which project is a session's own - `Project::own_of` of its owner, refused if the owner is no user - is worked out where it is needed, in the runtime and in its test fixtures, instead of being asked of the session.

## The problem

A session's own project (issue [467](467-projects-hold-what-outlasts-a-session.md)) is made by
`Project::own_of(session_id, owner)`. Where the session is at hand, its callers derive the owner
themselves, refuse an owner who is no user, and - for a new session - make the project the
session's:

- `SessionManager::create` (`caelumora-runtime/src/session/manager.rs:323`): the owner of the
  session's attributes, `as_user_id`, `SessionError::OwnerNotAUser`, then `session.project =
  own.id`;
- `SessionLoop::update_project` (`caelumora-runtime/src/session/settings.rs:158`), which makes the
  session an own project again once its own was lifted: the same derivation, without setting it;
- `session::testing::persist` (`caelumora-runtime/src/session/testing.rs:230`, issue
  [471](471-world-builds-sessions-by-hand.md)): the same three steps as `create`, an `expect` for
  the refusal.

`SessionTemplate::build` (`caelumora-model/src/template.rs:343`) makes it from the owner it is
given, a `UserId`, and needs no derivation.

## Why it matters

- A value derived from an entity is computed by the entity, so that its callers cannot disagree
  about it - "the model derives",
  [CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md#runtimes-and-their-entities). Which project
  is a session's own is such a value.
- A change to it - another owner than the session's, a project of a template - has to be found in
  three places.

## Considerations

- **A. A method of `Session` in the model**, e.g. `Session::own_project(&self) -> Option<Project>`
  - `None` if its owner is no user -, which each caller turns into its own error; `create` and the
  fixture set the project themselves. Small, and the model keeps no error of the runtime's.
- **B. A method that also makes it the session's**, e.g. `Session::take_own_project(&mut self)`,
  for the two callers that make a new session. Saves a line at each, but one of three callers
  needs the setting, and a method that changes the session and answers with another entity says
  two things.
- **C. A function of the runtime's `session` module** returning `Result<Project>` with
  `SessionError::OwnerNotAUser`. Keeps the error in one place, but derives in the runtime what
  belongs to the entity.

Recommendation: A.

## Done when

- Which project is a session's own is asked of the session; no caller derives it from the
  session's owner itself.

## History

- **2026-10-28** — Opened at the maintainer's request from the findings of issue 471.
