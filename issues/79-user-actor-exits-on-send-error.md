# 79 — `UserActor` exits on a send error, and the three actor run loops differ only there

**Status:** open
**Area:** Sessions & channels

## The problem

`UserActor::run` (`caelumora-runtime/src/actor_impl/user.rs:189-191, 254`)
ends the loop with `?` when forwarding an envelope fails. The user loses
their actor in the channel and nothing starts it again. `ApproverActor` logs
and carries on in the same situation (`approval.rs:335-337`); `AgentActor`
fails the turn.

The three run loops (`agent.rs:1077-1146`, `user.rs:234-264`,
`approval.rs:310-356`) are otherwise alike: `select!` over the inbox and
`ActorRuntime::subscribe`, the same "is starting/has finished" lines, the
same handling of a closed receiver.

## Why it matters

A transient error must not remove a participant. And three copies of one
loop drift - this is the drift.

## Considerations

- For the stateless actors (user, approver) one shared frame
  `run_reactive(rx, tx, on_envelope, on_global)` with "log and continue"
  everywhere. The agent keeps its state machine (its planning states
  consume no event). About 60 lines in two files.
- The fix alone (replace `?` with `warn!`) is two lines and can go first.

## Done when

- A failed forward is logged; the `UserActor` keeps running.
- User and approver share one run loop.

## History

- **2026-09-28** — Found in the structure review
  (`work/2026-09-28-code-review.md`, section 1, error 5 and 3.2.2).
