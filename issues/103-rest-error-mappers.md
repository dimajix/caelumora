# 103 — Six REST error mappers with one pattern

**Status:** done
**Area:** User interaction

## The problem

`session_error` (`routes/mod.rs:38`), `channel_error` (`channels.rs:84`),
`participant_error` (`:303`), `cancel_error` (`:369`), `command_error`
(`:413`), `message_context_error` (`:476`), plus an inline mapper in
`get_session` (`sessions.rs:92-99`) that rebuilds `session_error`. All do
the same: downcast the domain error → status and code, else fall back.
`channel_error` and `participant_error` have identical signature and
fallthrough; `cancel_error` and `command_error` repeat `is_forbidden →
user_message → forbidden`; `command_error:418` matches
`CommandError::Forbidden`, which `is_forbidden` (`runtime/error.rs:64-71`)
covers a line later. The pair `act`/`doing` ("add participants to"/"adding a
participant to") is threaded through three levels for one word.

## Why it matters

A new domain error variant has to be added to the right one of six
mappers, and a wrong status is a UI that shows the wrong message (the UI
matches on codes, e.g. `context_forbidden` in `ContextPanel.tsx:26`).

## Considerations

One `api_error(error, code_prefix, action) -> ApiError` in table form:
the forbidden family → 403; `NotFound`/`ChannelNotFound` → 404;
`AlreadyParticipant`, `DuplicateChannelName` → 409; other `SessionError`/
`ChannelError`/`CommandError` variants → 400; everything else →
`ApiError::internal`. Codes stay derivable from the variant so the UI's
matches keep working. About 80 → 35 lines, half a day with tests.

## Done when

REST has one function that maps a runtime error to an `ApiError`.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 5.2.2).
- **2026-09-29** — Done. `ApiError::from_runtime(error, subject, action)`
  in `crate::error` is the one place a runtime error becomes a status and a
  code: missing session, template, channel, participant or context → 404;
  one that exists already → 409; any other `SessionError`, `ChannelError`,
  `CommandError` or `TemplateError` → 400; a refusal (`is_forbidden`, a
  refused context) → 403 `<subject>_forbidden`; everything else →
  `ApiError::internal` as `<subject>_failed`. `Subject` (session,
  participant, cancel, command, context) only names those two codes, so
  the codes the web UI matches (`context_*`) and the others stay as they
  were. `session_error`, `channel_error`, `participant_error`,
  `cancel_error`, `command_error`, `message_context_error` and the two
  inline mappers in `sessions.rs` are gone, and so is the `act`/`doing`
  pair: one `action` phrase names what was done ("adding a participant to
  channel …"). Codes that changed, none of them matched by a client: a
  server error loading or creating a session is `session_failed` (was
  `session_unavailable`/`session_start_failed`); refusing to change a
  channel's participants is `participant_forbidden` whichever layer refused
  it (the store's refusal was `session_forbidden`). A test checks the
  table.
