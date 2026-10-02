# 187 — Deleting a channel may leave an external agent's remote tasks running

**Status:** open
**Area:** Sessions and channels

## The problem

`SessionLoop::delete_channel` calls `stop_actors()` and aborts the channel's task right after, with
no grace period. `remove_participant` waits `STOP_GRACE` before it aborts. An
`ExternalAgentActor` cancels its open remote tasks after its run loop ends, when it was stopped
(issue [179](179-external-agents-via-a2a.md)); if the abort reaches it first, the remote tasks keep
running after the channel is gone. Whether the abort reaches the actor's task, or only the channel
loop's, is **to be verified**.

## Why it matters

Remote work that nobody waits for any more keeps running, and may cost money on the remote side.
The deeper gap: `Actor::stop` is fire-and-forget, and only one of its two callers waits for the
actor to finish.

## Considerations

- Give `delete_channel` the same grace period as `remove_participant` - one shared way to stop a
  channel's actors and wait for them.
- Or let `stop` return something to wait on, so that every caller can.

## Done when

- Deleting a channel ends its external agents' open remote tasks, covered by a test.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series.
