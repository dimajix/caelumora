# 336 — What a plugin's error says is shown to nobody

**Status:** open
**Area:** Observability and operations

## The problem

Two places turn an `anyhow::Error` into what a person is told, both by the error types they know:

- `caelumora_runtime::error::user_message` (issue [02](02-better-error-messages.md)) - what an
  agent's owner, the user of a command or the TUI is shown when a turn, `/wakeup` or `/nudge`
  fails - downcasts the chain to the core's error types it knows to be safe to disclose, and
  answers `None` - "see the server log" - for anything else.
- `From<anyhow::Error> for ApiError` in the REST server answers the core's error types by their
  `From`, a `Refused` anywhere in the chain as `<subject>_forbidden` and a `NameTaken` as
  `409`; everything else as `500 internal_error`.

A plugin's error type is known to neither. Since step 10 of issue
[323](323-kinds-enumerated-in-the-core.md), `SkillError` is a plugin's: where it travels inside an
`anyhow` - a skill that cannot be deleted with its session, an error of the `skills` component
while a turn runs -, its message is shown as a failure. Only what it carries as its source is
still found: a `StoreError`, judged as store errors are, and a `Refused`. Before the move,
`user_message` knew `SkillError` and showed its other messages ("you are not cleared for the
classification of the skill", …).

## Why it matters

Every plugin loses this, and each further one - artifacts, issue
[328](328-artifacts-in-a-crate-of-their-own.md) - loses more. Cut 12 of issue 323 made an error
answered by what it is for the REST handlers that call the plugin directly; it did not reach the
chains.

## Considerations

- **A. A disclosable message as a source**, after the pattern of `Refused`: a type of the core -
  `Disclosed`, say, holding the message a person may read - that an error carries as its
  `#[source]` where its message is safe. `user_message` finds it in the chain as it finds a
  `Refused`. Cheap, and it extends the pattern cut 12 chose; the cost is a field in the variants
  that disclose, and a message written twice where `Display` already says it.
- **B. A trait the error implements** (`UserFacing::user_message`). Not findable in a chain of
  `&dyn Error` without a registry of downcasts, which every plugin would fill in when it
  registers - a second mechanism beside the chain.
- **C. Errors that are safe say so by a marker type wrapping them** (`Disclosable<E>`). Found by
  downcast only if the core knows `E`; does not help.

Recommendation: A, also for the REST server: a disclosed error without a type the server knows is
answered `400` or `409` by a kind the `Disclosed` names, rather than `500`.

## Done when

- A plugin's error that may be shown reaches the person through `user_message` and the REST
  server alike, whatever chain it travels in; `SkillError` uses it, and `docs/CODING-GUIDELINES.md`
  says how an error declares what may be shown.

## History

- **2026-10-12** — Opened from step 10 of issue 323: moving skills into a plugin left
  `user_message` without `SkillError`.
