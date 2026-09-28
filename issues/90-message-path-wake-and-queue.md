# 90 — Sending a message: the frontends wake the channel, the `UserActor` queues for nothing, and `perceive` begins a turn

**Status:** open
**Area:** Sessions & channels

## The problem

The path "a user writes, an agent answers" has three detours:

1. **Frontends wake the channel.** REST (`routes/channels.rs:329`) and the
   TUI (`tui/main.rs:304`) call `channel.wake_for_delivery()` before
   `send_message`; `send_approval` does not - an asymmetry without a reason.
   The channel sees the user's envelope itself; the commented-out block at
   `channel.rs:650-654` wanted to wake there.
2. **The `UserActor` queue is a hop without a purpose.** `user_tx`/`user_rx`
   (`actor_impl/user.rs:37-38`) exist only because the channel's inbox sender
   is created in `run` (`channel.rs:496`). `UserActor::run` does nothing but
   pass the queue on.
3. **`perceive` begins a turn.** Since `ed9abe8`, `perceive` calls
   `begin_turn` (`agent.rs:149`), which asks the `SecurityManager` twice per
   perceived message and agent, and resets the turn - although
   `request_response` does both again a moment later. The `PerceiveIn`
   processors probably need only the `EventContext` for provenance.

## Why it matters

Two frontends must know a precondition of the runtime (1); every user
message crosses one queue more than it has to (2); and a policy check per
perceived message scales with the number of agents in the channel (3).

## Considerations

- (1) The channel wakes on a user's `FullResponseGenerated`; since REST
  wakes on every message today, that is behaviour-preserving. The
  `channel_wakeup_failed` error path in REST goes.
- (2) `ChannelLoop::new` creates `event_tx` and offers
  `pub(crate) fn submit(&self, Envelope)`; `UserActor::send_message` calls it
  directly. `channel.rs` 2 places, `user.rs` 4 methods + 2 tests.
- (3) Check what the processors use; give `perceive` the event context
  without `next_turn`. `begin_turn` stays the one place a turn starts.
- With `Actor::wakeup`/`cancel` never failing and never awaiting, both can
  become plain `fn`, removing three `?` in `channel.rs` (see
  [107](107-public-interface.md)).

## Done when

- No frontend calls `wake_for_delivery`.
- `UserActor` has no queue of its own.
- Perceiving a message asks the policy nothing.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, path 3.4 A). Point 3 is a cost of
  commit `ed9abe8`, noticed by the review.
