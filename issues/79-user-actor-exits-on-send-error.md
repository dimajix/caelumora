# 79 — `UserActor` exits on a send error, and the three actor run loops differ only there

**Status:** done
**Area:** Sessions & channels

## The problem

`UserActor::run` (`caelumora-runtime/src/actor_impl/user.rs:189-191, 254`) ends the loop with `?`
when forwarding an envelope fails. The user loses their actor in the channel and nothing starts it
again. `ApproverActor` logs and carries on in the same situation (`approval.rs:335-337`);
`AgentActor` fails the turn.

The three run loops (`agent.rs:1077-1146`, `user.rs:234-264`, `approval.rs:310-356`) are otherwise
alike: `select!` over the inbox and `ActorRuntime::subscribe`, the same "is starting/has finished"
lines, the same handling of a closed receiver.

## Why it matters

A transient error must not remove a participant. And three copies of one loop drift - this is the
drift.

## Considerations

- For the stateless actors (user, approver) one shared frame `run_reactive(rx, tx, on_envelope,
  on_global)` with "log and continue" everywhere. The agent keeps its state machine (its planning
  states consume no event). About 60 lines in two files.
- The fix alone (replace `?` with `warn!`) is two lines and can go first.

## Done when

- A failed forward is logged; the `UserActor` keeps running.
- User and approver share one run loop.

## History

- **2026-09-28** — Found in the structure review (`work/2026-09-28-code-review.md`, section 1, error
  5 and 3.2.2).
- **2026-09-29** — Done, but not as described: the finding was wrong. The in-memory transport's
  `send` waits while the queue is full and fails only when the receiver is gone - the inbox of
  `ChannelLoop::run`, i.e. when the channel no longer runs. There is no transient error that would
  remove a participant; ending the actor then is right (its task would be aborted with the channel's
  `JoinSet` anyway). What was wrong was the difference: the `UserActor` reported that end as an
  error (`tracing::error!` via `?`), the `ApproverActor` logged a `warn!` and kept running for
  nothing. Both now end quietly with an `info!` when their output is closed, and log anything else
  as a warning and carry on. `InMemoryEventSender::send` reports a closed queue as
  `TransportError::Closed` like `try_send` does (it said `Backend`), and `transport::is_closed`
  finds it through any context. Test: a `UserActor` whose channel is gone ends with `Ok`. The shared
  run loop was not built: the two loops differ exactly in the parts that are not the frame - the
  user's second input queue and the approver's timeout timer. Once
  [90](90-message-path-wake-and-queue.md) removes the user's queue, it is worth looking at again;
  noted there. Should a remote transport ([21](21-scaling.md)) bring transient send errors, whether
  an actor survives them is decided with that transport.
