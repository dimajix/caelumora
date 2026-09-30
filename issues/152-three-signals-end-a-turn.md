# 152 — Three signals and a precedence rule end a turn

**Status:** done
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
- **2026-09-30** — Done as proposed.
  - `enum Interrupt { Cancel, Stop }` on the actor, kept in an `AtomicU8`;
    `cancel` leaves `Cancel` while the actor is in `Planning`,
    `ExecutingTools` or `WaitingForApproval` (never over a `Stop`), `stop`
    leaves `Stop`; both cancel the turn's token. The run loop takes the
    interrupt after every step: `Stop` gives the turn up and ends the loop,
    `Cancel` turns a next state within a turn into `Cancelling`. A cancel
    left over when the next turn begins is dropped there. `stopping`, the
    CAS in `cancel` and the precedence rule are gone.
  - `ActorState` is `#[repr(u8)]` with `ActorState::ALL`, so the holder's
    byte has no gaps; `ActorState::in_turn()` replaces
    `AgentActor::is_turn_state`; `ProcessingStateHolder::transition` is
    `replace(from, to)`, which `wakeup` uses.
  - New test: a cancel while a tool runs ends the turn after the tool
    without asking the model again (it fails without the rule). For it the
    built-in component `sleep_tool` (tool `sleep`, parameters `seconds`
    and `requires_approval`) was added at the maintainer's request, as a
    tool for tests generally.
  - Tested in the web UI with a personality that calls `sleep`: a cancel
    while it runs ends the turn once the tool is done, with its result and
    no further answer. A stop - removing the agent - while it runs found
    two gaps older than this change, opened as
    [161](161-tools-cannot-be-interrupted.md) and
    [162](162-ui-keeps-a-removed-agents-turn.md). CHANNELS.md describes the
    interrupt.
