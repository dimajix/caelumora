# 103 — Six REST error mappers with one pattern

**Status:** open
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
