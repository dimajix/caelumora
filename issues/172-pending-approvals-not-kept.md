# 172 — Pending approvals live only in the approver's memory and the stream

**Status:** open
**Area:** Tools and approvals

## The problem

An approval request waiting for its decision is kept in two places, and
neither lasts:

- **The approver holds it in memory** (`ApproverActor::pending`). The
  agent's turn is stored with its pending calls and resumes in
  `WaitingForApproval` after a restart (issue 75), but the approver starts
  empty: nobody is asked, no answer is taken, no timeout fires. The turn
  waits until it is cancelled.
- **A user interface knows it only from the stream** (`ApprovalRequested`
  from the approver). There is no way to ask a channel for its pending
  approvals; a reload of the web UI, or a client that connects later, does
  not show them, and the users cannot answer.

Found while implementing [148](148-cancelled-call-without-result.md).

## Why it matters

- A turn that waits for an approval nobody can give looks like a hanging
  agent; the only way out is a cancel.
- The longer an approval waits - the default timeout is ten minutes - the
  likelier a reload or a restart in between.

## Considerations

- **The agent's state is the record.** The pending calls are stored with
  the turn (`TurnState::tool_calls`). An approver could be given them again
  when the agent resumes - the agent asks again with the same ids - and a
  channel could answer the pending approvals from its agents' turns.
- **The approver keeps its own state** in the `StateStore`, with the
  answers given so far and the timeout. Survives a restart with the
  answers, but is a second record of the same requests.
- Either way the channel snapshot (`GET …/channels/{cid}`) would carry the
  pending approvals, so that a client shows them after a reload.
- Related: [04](04-postgres-state-store.md) (a store that survives the
  instance), [122](122-several-instances.md).

## Done when

- After a restart, a turn waiting for an approval can be decided.
- A client that connects to a channel shows its pending approvals.

## History

- **2026-10-01** — Opened while implementing
  [148](148-cancelled-call-without-result.md).
