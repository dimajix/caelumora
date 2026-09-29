# 88 — Template instantiation does store I/O in the model, and both frontends drive it by hand

**Status:** done
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
- **2026-09-29** — Done, as agreed with the maintainer. The model makes no
  store call any more: `ParticipantTemplate::request(session, owner)` says
  what a participant is for a session (`ParticipantRequest`), and
  `SessionTemplate::build(id, owner, name, participants)` builds the
  session and its channels from the participants it is given.
  `SessionContext` is gone, and so is the `mapping`: the template's "user"
  is the session's owner, another user participant is
  `TemplateError::UnboundUser` - no frontend passed more, and naming
  members at creation would be a parameter later. The runtime resolves:
  `SessionManager::create_from_template(ctx, template_id, name)` reads the
  template, checks the owner exists, finds or makes the agents
  (`caelumora_runtime::template`, with the rule of issue 77) and creates
  the session; REST and the TUI call only that. REST answers a missing
  template with 404 and an invalid one with 400 (`template_invalid`)
  instead of 500. Approvers, on the maintainer's wish, stay in the
  template as ordinary named participants with a `strategy` (default
  `any`), so a template lists every actor of its session; the magic
  channel participant "approval" is gone, `ParticipantTemplate::name` is
  always a name, and `Channel::build` keeps adding an approver to a
  channel that has none - one guarantee for channels from templates and
  on request. `ScopeTemplate::GroupScope` is `TemplateError::
  UnsupportedScope` instead of a `todo!()` panic. `TemplateError::Store`
  is gone; store errors are the runtime's. The setup templates of REST
  and TUI name their approver.
