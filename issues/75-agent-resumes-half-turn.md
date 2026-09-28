# 75 — A resumed turn cannot tell a crash from a deliberate stop, and re-runs its tools

**Status:** done
**Area:** Sessions & channels

## The problem

`AgentActor::new` takes the stored `processing_state` over
(`caelumora-runtime/src/actor_impl/agent.rs:116`), and `run` continues from
it: at `Planning`, `ExecutingTools` or `Reflecting` it re-enters the turn
with the stored working state. This is by design - after a server crash an
agent is meant to carry on exactly where it stopped, which is why the
turn's context, messages, tool calls, plan and (since `ed9abe8`) its
publication and classification are all stored.

What the design does not yet distinguish:

- **A deliberate stop looks like a crash.** `remove_participant` aborts the
  actor mid-turn; its state was stored after the last step. If the agent is
  added again later, it resumes a turn from a membership that ended - and
  records its messages into a channel it had left in between. The same
  holds for a turn cut off by `cancel` if the state was stored before the
  cancel took effect.
- **`ExecutingTools` re-runs the tools.** A resume in that state repeats
  every tool call that had no result yet, including those that had started.
  That is the case issue [20](20-transactional-effects.md) describes.
- **`Planning` repeats the model call**, which is the intended behaviour and
  fine - but nothing says so, and nothing marks the resumed step in the log.

## Why it matters

Resuming is the valuable property (issue [04](04-postgres-state-store.md)
calls a suspended agent the most expensive state to lose). It stays
valuable only if a resume never happens where nothing should be resumed,
and never re-executes an effect.

## Considerations

- **Name the two ends of a turn.** A deliberate end (`remove_participant`,
  `stop_tool`, `cancel` once it has taken effect) abandons the turn *before*
  the state is stored; only a crash leaves a turn under way in the store.
  Then "a stored turn" means "resume", without a flag.
- **Per state, say what a resume does.** `Planning`/`Reflecting`: continue.
  `WaitingForApproval`: continue. `ExecutingTools`: until 20 gives tools a
  dispatch record, resume as `Planning` with the tool calls discarded, so
  that the model decides again rather than the runtime re-running an
  effect blindly. Write it down at `run`.
- **Make the turn one value.** Issue
  [85](85-turn-state-in-agent-channel-state.md) stores the working state as
  `Option<TurnState>`; a stored `Some` is then exactly "a turn to resume",
  and `abandon_turn` before a deliberate stop leaves `None`.
- Rejected: normalising every stored state to `Idle` on load. That was the
  review's first reading and would remove the property this issue is
  about.

## Done when

- A participant removed mid-turn and added again starts idle.
- A crash during `Planning`, `Reflecting` or `WaitingForApproval` resumes
  the turn; a crash during `ExecutingTools` does not re-run a tool.
- The resume rules are documented at `AgentActor::run` and in
  ARCHITECTURE.md.

## History

- **2026-09-28** — Found in the structure review
  (`work/2026-09-28-code-review.md`, section 1, error 1 and path D). The
  review read the resume as a bug; the maintainer clarified that resuming
  after a crash is the intent. The issue was rewritten the same day to what
  remains: distinguishing a deliberate stop from a crash, and not re-running
  tools.
- **2026-09-28** — Done. `Actor::stop` (synchronous, like `cancel`) is what
  `remove_participant` calls: the agent cancels a model call in flight,
  abandons a turn under way at the next step boundary, stores itself idle
  and ends its task; the channel closes its inbox and aborts it only after
  a grace period (`STOP_GRACE`, 5 s) if it does not end by itself. `cancel`
  and the stop tool already committed their turn, so only a crash leaves a
  turn in the store. On load, `AgentActor::prepare_resume` applies the
  resume rules; every state resumes as it is except `ExecutingTools`, where
  approved calls without a result are answered as not executed
  (`crate::tool::not_executed_result`, status `Failed`) so that the model
  decides again - instead of the "resume as `Planning` with the tool calls
  discarded" considered above, which would have left the assistant message
  with the calls without results and the staged changes of finished calls
  orphaned. Tests: a removed agent leaves an idle state with no turn
  behind and announces nothing; the resume rule on an `ExecutingTools`
  state.
