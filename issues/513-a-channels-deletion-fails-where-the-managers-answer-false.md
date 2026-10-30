# 513 — A channel's deletion fails where every manager's deletion answers `false`

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `SessionLoop::delete_channel` answers a channel that is not there, or not the caller's to read, with `SessionError::ChannelNotFound`, while every manager's deletion - the session's among them since issue 510 - answers the same case with `Ok(false)`.

## The problem

Since issue [510](510-a-sessions-purge-fails-where-the-others-answer-false.md), every manager's
`delete` and `purge` answer "nothing there the caller could see" with `Ok(false)`, and their REST
handlers turn `false` into a 404 of their own (`docs/REST-SERVER.md`). `SessionLoop::delete_channel`
(`caelumora-runtime/src/session/channels.rs`) returns `Result<()>` and fails with
`SessionError::ChannelNotFound` where the session has no such channel or the caller may not read
it or is not cleared for it (`listed_channel_for`); its handler (`routes/channels.rs`,
`delete_channel`) passes the error on.

## Why it matters

Similar things are to fail the same way. A channel's deletion is the one deletion left that tells
"not there" by an error, so a caller that deletes channels and sessions alike handles the one as an
answer and the other as an error.

## Considerations

- **It answers `bool`.** `delete_channel` returns `Result<bool>`, `false` where
  `listed_channel_for` finds nothing the caller may reach; its handler answers `false` as
  `404 channel_not_found`. Its callers - the handler, the session's own uses, the tests - assert
  or map the answer.
- **Leave it.** `delete_channel` is no manager's operation but the running session's, which finds
  its channel by the caller first, as every operation of `SessionLoop` does; that it fails like
  them, not like the managers, may be the line to keep.

## Done when

- A channel's deletion answers "nothing there the caller could see" as the managers' deletions do,
  and its REST handler maps it the same way - or the difference is named, with its reason, where
  the managers' answer is described.

## History

- **2026-10-30** — Opened from the findings of issue
  [510](510-a-sessions-purge-fails-where-the-others-answer-false.md), whose review noticed the
  channel's deletion as the one left that fails.
