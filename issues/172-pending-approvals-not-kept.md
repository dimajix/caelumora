# 172 — Pending approvals live only in the approver's memory and the stream

**Status:** done
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
- **2026-10-01** — Done, as decided with the maintainer: the agent's turn
  is the record, the approver stays in memory.
  - An agent that resumes a turn with calls still pending asks again with
    the same ids (`TurnState::pending_approvals`, `AgentActor::run`) - not
    only in `WaitingForApproval`: a turn stored right after asking resumes
    executing its tools, and waits after that. A cancelled one does not; it
    withdraws.
  - The approver takes a request it still holds as announced again, keeping
    the answers and the timeout. After a restart it holds nothing: the
    timeout starts anew, and answers given before are lost - the users are
    asked again.
  - The pending approvals are read from the agents' stored turns
    (`ChannelView::pending_approvals`) for a viewer who takes part in the
    channel as a user, and sent with the channel stream's snapshot
    (`pending_approvals`) rather than in the channel snapshot of
    `GET …/channels/{cid}`, as proposed above: that one is taken for every
    channel of every session listed, and reading every agent's state there
    is not worth it; the stream's snapshot also leaves no window between
    reading and listening.
  - The web UI takes them from the snapshot and shows a request asked
    again once. Whether a user already answered is known only to the page.
