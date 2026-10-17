# 187 — Deleting a channel may leave an external agent's remote tasks running

**Status:** done
**Area:** Sessions and channels
**Summary:** Deleting a channel aborts it without a grace period, which may leave an external agent's remote tasks running.

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
- **2026-10-02** — Verified: the abort did reach the actors. Their tasks live in the `JoinSet` of
  `ChannelLoop::run`, which is dropped with the aborted channel task, so the external agent's
  cleanup never ran. A second consequence: an actor given time to end could store its state after
  `delete_channel_state` had deleted it.

  Rejected: stopping the channel through an `ActorChange` handled by `run` - a change queued there
  waits behind whatever `run` is busy with, and `stop` and `cancel` go past the events for exactly
  that reason. Giving up the `JoinSet` for plain `JoinHandle`s was rejected too: the actors would
  then no longer end with an aborted channel, which stopping a session relies on.

  Done: every actor's task is tracked by a `TaskTracker` (tokio-util) in `ChannelLoop::actors`,
  inside the task the `JoinSet` owns. `stop_actors` stops every actor and waits on the trackers at
  most `STOP_GRACE`; `delete_channel` awaits it before it aborts the channel's task and deletes the
  state. `remove_participant` waits on the same tracker instead of sleeping. `Actor::stop` has no
  default any more, and the `ApproverActor` ends its run when stopped. Covered by
  `a_stopped_channel_waits_for_its_external_agent_to_cancel_its_open_task`.
