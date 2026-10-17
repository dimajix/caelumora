# 102 — The REST server looks a channel up twelve times the same way, and answers access questions itself

**Status:** done
**Area:** User interaction
**Epic:** [74](74-refactoring-epic.md)
**Summary:** The REST server repeats the session/channel lookup twelve times and decides itself what a viewer may see.

## The problem

`ensure_session` + `channel_of` appear as a pair twelve times in `routes/channels.rs` (103/105,
137/138, 154/155, 201/202, 274/276, 290/292, 325/327, 356/360, 385/386, 400/401, 436/437, 494/496),
six of them followed by `user_actor_of`; names (`session` vs. `session_loop`) and parameters
(`channel_id` vs. `ChannelId(cid)`) vary, and `user.security_context()` is rebuilt up to three times
per handler. `channel_of` asks `may_read_channel` itself (`routes/mod.rs:58`) - the server, not the
runtime, decides what a viewer sees. `cancel_channel` (`channels.rs:361`) additionally requires
participation through `user_actor_of` before the runtime checks `CancelChannel` - a second access
decision in the handler, against the rule of issue [71](71-access-helpers-on-entities.md).
`SessionLoop::cancel_channel` (`session.rs:261`) exists only for that handler, which already holds
the channel; its error is `anyhow!` and its parameter order is reversed against every other method.

## Why it matters

Twelve copies of a lookup are twelve places to get the access check wrong, and the global snapshot
(`events.rs:48-95`) pays one policy call per session × channel for the same question.

## Considerations

- Runtime: `SessionLoop::channel_for(ctx, id) -> Result<Arc<ChannelLoop>>` as the counterpart of
  `SessionManager::open_session`, returning `NotFound` when `may_read_channel` says no;
  `SessionLoop::readable_channels(ctx)` for `list_channels` and `describe_session`. Then the server
  asks no access question itself and `may_read_channel` in `routes/mod.rs` goes.
- Server: `open_channel(state, user, id, cid)` and `acting_user(…)`; each handler loses two or three
  lines.
- Remove `SessionLoop::cancel_channel`; the handler calls `channel.cancel(&ctx)`. Whether only
  participants may cancel is a policy question (`AccessQuestions`/`ActionType::CancelChannel`), not
  a handler check.
- `AuthenticatedUser::actor_id()` replaces the three spellings of the viewer (`initiator_id`,
  `viewer_id(&user)`, `ActorId::User(uid) == me`).

## Done when

- No REST handler calls `may_read_channel`; the runtime returns only what the viewer may see.
- The session/channel lookup is one call.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 5.2.1, 5.2.5, 5.5
  "Cancel").
- **2026-09-29** — Done, as agreed with the maintainer. The runtime decides what a viewer sees:
  `SessionLoop::channel_for(ctx, id)` returns a channel only if `ctx` may read it (`GetChannel`
  through `Enforcement::require`, answered as `SessionError::ChannelNotFound` like a missing one),
  and `readable_channels(ctx)` filters the channel list and the session snapshot. REST reaches a
  channel through one call, `open_channel(state, user, session, channel)`; `channel_of` and
  `routes::may_read_channel` are gone, and no handler asks an access question itself. Cancelling is,
  in the maintainer's words, a meta-operation like renaming or deleting: the handler calls
  `channel.cancel(ctx)`, whose own `CancelChannel` check decides, and no longer requires
  participation through `user_actor_of`; `SessionLoop::cancel_channel` is gone. Taking part -
  writing, approving, commands, following live - stays with the user's actor (issues 90, 104).
  `AuthenticatedUser::actor_id()` names the viewer. A test checks that a channel its viewer may not
  read is neither found nor listed.
