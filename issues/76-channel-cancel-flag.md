# 76 — The channel's cancel flag suppresses the final response and is a third state machine

**Status:** open
**Area:** Sessions & channels

## The problem

`ChannelLoop::processing_state: Arc<AtomicU8>` (`caelumora-runtime/src/channel.rs:74`)
knows `Active` and `Cancelling`. Its only effect: while `Cancelling`,
`route_event` (`channel.rs:656-661`) suppresses *every* internal forwarding -
including the `FullResponseGenerated` with which the cancelled turn ends.
The history gets it (`append_history` runs before), the transcripts of the
other agents do not; they diverge until the next `wake_for_delivery`.

The same fact is already tracked twice: `AgentActor::processing_state`
(`ProcessingStateHolder`, stored in `AgentChannelState`) and
`AgentChannelState::requested_state`. `ActorState::Active` exists only for
the channel flag (`caelumora-model/src/actor.rs:324-326`), and so do the
public `From<u8>`/`From<ActorState> for u8` conversions.

## Why it matters

A divergent transcript is a wrong context for every later turn of the other
agents. And three state machines for "cancelled" are two too many to keep
in step.

## Requirement that must survive

`cancel` is called from another task or thread, asynchronously to the turn,
and must reach the running model call as fast as possible. Today that path
is the agent's atomic: `ChannelLoop::cancel` calls `AgentActor::cancel`,
which flips `processing_state` to `Cancelling` (`agent.rs:1194`) without
touching the turn; the streaming loop polls it between chunks
(`agent.rs:891`) and returns. The channel's flag is *not* on that path - it
only gates routing afterwards. Whatever changes here keeps the atomic as
the cross-thread signal and must not add an `await` or a lock in front of
it.

## Considerations

- **Drop the channel flag.** `cancel` checks the policy once for the channel
  (as today, one decision for the whole channel - issue 30) and calls
  `actor.cancel()` for each actor; routing is never suppressed. Remove
  `ActorState::Active` and make the `u8` conversions private to
  `ProcessingStateHolder`. Cost: `channel.rs` 5 places, `agent.rs` 2, model 1.
- **Reach the model call itself.** Polling between chunks means a cancel
  waits for the next chunk, or for a stalled request, however long that
  takes. A `CancellationToken` (or the `Cancelling` atomic wrapped as one)
  passed into `LLMClient::chat_completion` lets the adapter abort the HTTP
  request. That is the "down to the LLM" half and worth doing in the same
  change or right after; it needs a small change to the `LLMClient` trait.
- `Actor::cancel` and `Actor::wakeup` never fail and never await; both can
  be plain `fn`, which also keeps the fast path obviously synchronous
  ([107](107-public-interface.md)).
- Rejected: keeping the flag but letting the final response through. Fixes
  the divergence, keeps the third machine.

## Done when

- After `cancel`, every agent's transcript contains what the history
  contains.
- `ChannelLoop` has no processing state of its own; `ActorState::Active` is
  gone.
- A cancel interrupts a model call that is in flight, not only the next
  chunk; the path from `cancel` to the atomic has no `await`.

## History

- **2026-09-28** — Found in the structure review
  (`work/2026-09-28-code-review.md`, section 1, error 2 and 3.1.1). The
  maintainer added the requirement that a cancel is asynchronous from
  another thread and must reach the model call quickly; recorded above.
