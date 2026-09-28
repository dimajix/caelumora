# 88 — Template instantiation does store I/O in the model, and both frontends drive it by hand

**Status:** open
**Area:** Code structure

## The problem

`ParticipantTemplate::instantiate` (`caelumora-model/src/template.rs:210-253`)
reads users, looks agents up by name and creates them; `SessionContext`
(`session.rs:159-164`) exists to carry `Arc<dyn EntityStore>` and a
`SecurityContext` into the model for it. The REST handler
(`routes/sessions.rs:13-53`) and the TUI (`tui/main.rs:406-414`) both build
`SessionContext` + the `{"user" → owner}` mapping + `instantiate` by hand.
The store has no `get_session_template(id)`, so the REST handler lists all
templates and filters (`sessions.rs:20-24`).

## Why it matters

Find-or-create against a store is orchestration; in the model it cannot be
tested without a store and produced the swallowed error of
[77](77-template-swallows-store-errors.md). Two frontends repeating the
sequence is a third copy waiting to happen.

## Considerations

- The model yields the *request* from a template (name, scope,
  personality id); a `SessionManager::create_from_template(ctx, template_id,
  name)` resolves it against the store. `SessionContext` goes away; the
  frontends call one method.
- `EntityStore::get_session_template(id)` alongside the by-name lookup - or
  as part of [93](93-entity-store-dialects.md).
- The three ways to get an approver (magic name `"approval"` in
  `ChannelTemplate::instantiate`, `ParticipantTemplate::Approver`, and
  `Channel::build` adding one anyway) reduce to `Channel::build`; then
  `ParticipantTemplate::name()` is always `Some`.

## Done when

- `caelumora-model` performs no store call while instantiating a template.
- REST and TUI create a session through one runtime method.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.1.7 and 3.4 G).
