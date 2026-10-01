# 148 — A cancelled turn takes over tool calls that have no result

**Status:** done
**Area:** Tools and approvals

## The problem

A turn cancelled while tool calls are pending - waiting for an approval -
is taken over (`handle_cancelling` → `commit_turn`) with the assistant
message that made the calls, but without a tool message answering them:
the calls' states (`TurnState::tool_calls`) are dropped with the turn. The
agent's transcript then holds calls that were never answered.

Seen with [141](141-turn-during-reclassification.md): since waiting for an
approval ends on a cancel at once, a transcript after `/cancel` during an
approval holds `[Assistant with 1 tool call, no result]`. It happened
before as well, when the approval came after the cancel.

## Why it matters

- Model APIs expect every tool call to be answered by a result; a
  transcript with an unanswered call can make the next model call fail,
  or confuse the model about what happened.
- The record says a call was made, but not that it was never run.

## Considerations

- **Answer them when taking over.** `commit_turn` answers every call
  without a result, as `prepare_resume` does after a restart, with a
  result saying it was not executed because the turn was cancelled, and
  appends the tool message before taking the turn over.
- **Leave it to the profile.** The model's profile drops unanswered calls
  when the context is given. Keeps the record as it is, but every backend
  would have to know.
- A turn given up ([145](145-ended-turns-leave-a-record.md)) has the same
  shape on its dead branch; there it is only a record, but the same
  answer would make it readable.

## Done when

- No transcript holds a tool call without a result after a turn ended.

## History

- **2026-09-30** — Found while implementing
  [141](141-turn-during-reclassification.md), and confirmed with a probe
  in the test `a_cancel_ends_a_turn_waiting_for_approval_at_once`.
- **2026-10-01** — Calls without a result are answered when their turn
  ends (the first consideration). The open step had a further gap: the
  results of calls that already ran sit in `TurnState::tool_calls` until
  every call of the step has one, so a cancel lost them as well, and a
  turn given up did not tell them (`executed_calls` only looks at the
  messages). Now `Turn::commit_turn` and `give_up_turn` first end the open
  step (`ToolCalls::abandon`): every call without a result is answered
  with the new status `NotExecuted`, saying why the turn ended, and the
  tool message is kept with the results of those that ran. The calls
  answered after a restart (issue 75) take the same status instead of
  `Failed`. Still open, and to be done in this issue: the approvals of
  such calls stay pending at the approver and in the user interface after
  the cancel.
- **2026-10-01** — The approvals of the calls answered so are ended too.
  The agent withdraws its requests still waiting for a decision when its
  turn ends (`AgentActor::withdraw_approval_requests`, sending
  `ActorEvent::ApprovalRequestWithdrawn`); the approver takes the withdrawal only from the agent that asked, forgets
  the request and tells the users. A stopped agent withdraws nothing: the
  approver answers its requests as it leaves. Users also learn of every
  decision of the approver now - before, only the agent did, and the web
  UI dropped an approval as soon as any user had answered, though under
  `All` or `Majority` it was not decided yet. The web UI and the TUI drop
  an approval only once the approver ends it. That pending approvals are
  lost on a restart or a reload went to
  [172](172-pending-approvals-not-kept.md). Done.
