# 148 — A cancelled turn takes over tool calls that have no result

**Status:** open
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
