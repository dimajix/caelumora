# 81 — The approver issues two ids per approval, and the agent detours through `ExecutingTools` to wait

**Status:** done
**Area:** Tools & approvals

## The problem

`ApproverActor::handle_request` (`caelumora-runtime/src/actor_impl/approval.rs:110-126`) assigns a
new `ApprovalId` to every request and keeps the original in `PendingApproval::_request`, which is
never read. The rewrite protects nothing: the moderator routes user answers to the approver anyway
(`moderator_impl/mod.rs:210-215`). `handle_timeouts` (`:266-275`) builds the decision envelope by
hand instead of calling `send_decision(p, Timeout, …)`.

On the agent's side, a tool call that needs approval goes `Planning → ExecutingTools` (because
`has_tool_calls`) → `execute_tools` sees `Pending` → `WaitingForApproval` → `ExecutingTools`.
`request_approvals` already knows whether anything is pending and could return `WaitingForApproval`
directly.

## Why it matters

Two ids for one approval are a trap for anyone reading logs or the context panel; the unused field
says the rewrite was never needed. The detour is a state transition with no work in it.

## Considerations

- One id: the agent's `ApprovalId` is the approval's id. `PendingApproval` loses two fields.
  `handle_timeouts` calls `send_decision`.
- `request_approvals` returns the next state; `handle_executing_tools` no longer needs to detect
  `Pending`.

## Done when

- An approval has one id from request to decision.
- An agent with a pending approval goes from `Planning` to `WaitingForApproval` in one step.

## History

- **2026-09-28** — Found in the structure review (`work/2026-09-28-code-review.md`, section 1, error
  7 and path 3.4 B).
- **2026-09-29** — Done for the ids. The approver keeps a pending approval under the agent's own
  `ApprovalId` and passes the request on unchanged; `PendingApproval` lost `original_approval_id`
  and `_request`, and `handle_timeouts` sends through `send_decision`. What keeps users from
  deciding for themselves is the moderator, which routes their answers to the approver only
  (`route_approval_events`), not a second id - now said at `PendingApproval`. The existing approver
  tests pass unchanged; the first asserts that the id stays the same. The second half was not done,
  deliberately: going from `Planning` to `WaitingForApproval` directly. The step through
  `ExecutingTools` is empty only when *every* call of the batch waits for approval; in a mixed batch
  it runs the calls that need no approval right away while the others wait. Returning
  `WaitingForApproval` directly would need exactly that distinction as a branch, to save one empty
  step - not worth a second path.
