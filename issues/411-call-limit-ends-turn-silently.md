# 411 — A turn that reaches the limit of model calls ends without a word

**Status:** done
**Type:** bug
**Priority:** high
**Area:** User interaction
**Summary:** When a turn has asked its model `MAX_MODEL_CALLS` times, it ends as if it were done - no message, no reason, only a warning in the server's log - so the users see an agent stop in the middle of its work, and a plan under way is lost with the turn.

## The problem

Before every model call, `AgentActor::handle_planning` counts the call (`Turn::count_model_call`,
issue 157). At `MAX_MODEL_CALLS` (25) it goes to `Reflecting`, and the turn ends like any finished
turn: `TurnFinished`, with whatever final messages there are - for a turn in a plan step, none.
The only trace is `warn!("Agent … asked its model 25 times in one turn; the turn ends")` in the log.

Seen on 2026-10-17 in session `s2`: Jonathan translated a Python program to Node.js in a plan of
nine steps. At step 6 he had made 24 calls; the context was compacted (issue
[284](284-automatic-context-compaction.md)), he made his 25th call - a `list_files` to see what was
written - and 0.2 s after its result the turn ended. The channel shows his last tool result and
then nothing; Jonathan is `Idle`. The plan was the turn's (`Turn::plan`) and is gone with it.

## Why it matters

*   The users cannot tell an agent that finished from one that was cut off, nor why it stopped. It
    looks as if the agent gave up or broke - the same problem issue
    [200](200-chain-limit-hint.md) solved for the end of a chain between agents.
*   Whoever wants it to go on has to guess that a nudge helps, and the agent then works without its
    plan, from what its context still holds.
*   The limit is a safety net against a model that loops; hitting it is not a normal end.

## Considerations

*   **A turn given up.** The runtime has a way to end a turn that did not finish: `TurnGivenUp` with
    a `GivenUpReason`, which the channel's history keeps, the web UI shows, and after which the
    agent may be asked to answer anew (issue 259). A reason of its own - the call limit, with the
    number - fits it; whether the turn's messages so far stay as they are, as for `Failed`, is to be
    checked against what `Failed` does with a plan under way.
*   **A notice beside the end.** Like `ChainCutOff` (issue 200), a notice in the system's name that
    the turn reached its limit, the turn ending as now. Says it, but keeps two ways for a turn that
    did not finish.
*   **What becomes of the plan.** Asked anew, the agent begins a new turn without the plan. Whether
    a plan should outlive its turn is issue [412](412-plan-budget.md); this issue only makes the
    end visible.

Recommended: a `GivenUpReason` of its own - one way for every turn that did not finish.

## Done when

*   A turn that reaches the limit is given up with a reason that says so, shown in the chat and kept
    in the history; the agent may be asked to answer anew.
*   A test ends a turn at the limit and checks what the users are told.
*   [AGENT-TURN.md](../docs/AGENT-TURN.md) says what happens at the limit.

## History

- **2026-10-17** — Opened after Jonathan in session `s2` stopped in the middle of a plan; the cause
  was found in its stored turn: 25 model calls, and the turn ended without a message.
- **2026-10-17** — Done as recommended: a turn that would make one more model call is given up as
  `GivenUpReason::CallLimit { calls }` - before its context is compacted for a call it will not
  make. Its users are told (chat, TUI, history), the model is told why the turn ended, and the
  agent may be asked to answer anew (`may_be_answered_again`; in the web UI
  `mayBeAnsweredAgain`). As for every turn given up, nothing it staged is applied - memories and
  notes noted in it are lost with it, as with `Failed`. The plan is lost too; that is issue
  [412](412-plan-budget.md). `Turn::count_model_call` is split into `may_call_model` and
  `count_model_call`.
