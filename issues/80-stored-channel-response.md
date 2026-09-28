# 80 — A stored channel is described as not changeable, and the response builders are scattered

**Status:** open
**Area:** User interaction

## The problem

`stored_channel_response` (`caelumora-rest-server/src/routes/events.rs:127-140`)
answers `can_change: false` and `can_read_context: false` for a channel that
is not running - false instead of unknown. Both are answerable from the
`Channel` document: `can_read_context` is
`may_read_agent_context(viewer, &agent.security_attributes)`, `can_change`
is `may_change_channel(viewer, &channel.security_attributes)`. The running
variant `channel_response` (`routes/channels.rs:117`) asks the actors for
what the participants already say (`kind` is the literal `"agent"`, the
`handle` is computed twice identically).

Around it, the response construction is spread over three modules:
`channel_response` and `knowledge_response` in `channels.rs`,
`stored_channel_response`, `session_snapshot`, `describe_participant` and
`describe_session` in `events.rs` - which is why `routes::channels` is the
only `pub` route module and `sessions.rs:100` imports its response shape
from the events module. `models.rs` already has the pattern (`From<PublishedNotes>`,
`From<SeenItem>`).

## Why it matters

The web UI hides the "change" and "context" controls for a channel that the
viewer may in fact change. And a builder per module means a new field is
added in two places.

## Considerations

- One `ChannelResponse::describe(&Channel, &dyn SecurityManager, viewer)`
  in `models.rs`, used for stored and running channels alike (the running
  one through `ChannelLoop::channel()`). Caveat: actor names follow the
  controller, participant copies do not - irrelevant until agents can be
  renamed.
- Move all `*_response`/`describe_*` into `models.rs` with a
  `(&dyn SecurityManager, viewer, …)` signature instead of `&AppState`;
  `routes::channels` becomes private again.

## Done when

- A stored channel reports `can_change`/`can_read_context` from the policy.
- One function describes a channel; all response builders live in `models.rs`.

## History

- **2026-09-28** — Found in the structure review
  (`work/2026-09-28-code-review.md`, section 1, error 6; 5.1.2 and 5.1.3).
