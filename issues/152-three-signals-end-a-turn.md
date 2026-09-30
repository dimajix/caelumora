# 152 — Three signals and a precedence rule end a turn

**Status:** open
**Area:** Code structure

## The problem

A turn is ended from outside by three signals that the run loop of
`AgentActor` reads apart:

- `cancel()` (`actor_impl/agent.rs`) switches `processing_state` to
  `Cancelling` with a CAS loop (`ProcessingStateHolder::transition`) and
  cancels the turn's token;
- `stop()` sets `stopping: AtomicBool` and cancels the token;
- `wakeup()` switches `Sleeping` to `Idle`.

`processing_state` thus has three writers - the loop, `cancel`, `wakeup` -
and the loop has to guess the one in between: "if the state is
`Cancelling` now and the step that ran was not, ignore what the step
returned". `stopping` is checked separately after every step.

`ProcessingStateHolder` (`caelumora-model/src/actor.rs`) maps `ActorState`
to bytes with gaps (1 and 4, states that are gone) and offers `transition`,
which after this change only `wakeup` needs. Whether a state belongs to a
turn is `AgentActor::is_turn_state`, a question about `ActorState` asked
four times in the actor.

## Why it matters

The end of a turn is what issues 141, 145, 146 and 148 keep changing. A
precedence rule over two flags is where the next change goes wrong - the
stop while waiting for an approval that 141 found was one.

## Considerations

- **One signal:** `enum Interrupt { Cancel, Stop }` on the actor, `Stop`
  winning over `Cancel`. `cancel()` sets `Cancel` while a turn runs and
  cancels the token, `stop()` sets `Stop` and cancels the token. The loop
  takes the signal after every step and alone decides: `Cancel` - next
  state `Cancelling`; `Stop` - give the turn up, store, end. The
  precedence rule, the CAS in `cancel` and `stopping` go.
  `handle_waiting_for_approval` stays: it returns on the token and the loop
  finds the signal.
- `wakeup` stays a change of state: the moderator and the channel read
  `Sleeping`. It becomes a `compare_exchange(Sleeping, Idle)`.
- `Cancelling` stays a stored state, so that a crash between the cancel and
  taking the turn over resumes by ending the turn, as today.
- With it: `#[repr(u8)]` on `ActorState` and a `from_raw` without gaps;
  `transition` goes. `ActorState::in_turn()` instead of
  `AgentActor::is_turn_state`.
- Costs one enum (review goal B), removes two fields, a CAS loop and a
  rule.

## Done when

- Only the run loop moves `processing_state` within a turn; `cancel` and
  `stop` only signal.
- There is no rule that discards what a step returned.

## History

- **2026-09-30** — From the review of the agent actor
  (`work/2026-09-30-code-review-agent.md`, A1, M3, M4), epic
  [151](151-agent-refactoring-epic.md).
