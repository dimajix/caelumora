# 419 — Who approved a tool call is recorded nowhere

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Tools and approvals
**Summary:** Once a tool call was approved, nothing stored says who approved it or by which rule, so neither the chat nor the record of the turn can show it.

## The problem

An approval goes from the agent's request to the approvers, from them to the users who may approve,
from the users back to the approver, which decides by its strategy and tells the agent and the
users (`route_approval_events`, issues 81, 148). Then the call runs. What is stored of that is the
call's status while it waits (`ToolCallStatus::WaitingApproval { id, description, explicit,
suggested, questions }`) and, once decided, `Ready` - with the answers to its questions
(`Answers { questions, answered_by, answered_by_name, responses }`, issue 165), but not who
approved it. The decision the approver sends (`ApprovalResponse { id, agent_id, by_rule }`) names a
rule only where a user's own rule answered, and the users whose answers made the decision stay in
the approver's `PendingApproval::responses`, which is gone once it decided.

So the chat shows a call as approved without by whom, and the record of the turn - the tool
message in the transcript and the history - cannot say it either. The only trace is the log.

## Why it matters

- A reader of the chat sees that a call ran which needed a person's consent, and not whose. That
  is a gap in the UI as much as in the record.
- An audit of approvals ([140](140-audit-log.md)) records the act - who, what, when - without
  content; the reader of the message needs the fact at the message. Both have their place, and
  neither can stand in for the other: the audit log is an administrator's, the message is its
  readers'.

## Considerations

### At the result, as the answers are

`ToolCallResult::answers` is the precedent: the questions a call asked and how they were answered
are kept with the result, "for the record - kept with the history and the transcript, published
as `content` is, never given to the model". The approval goes beside it:

```rust
pub struct ToolCallResult {
    …
    /// Who approved the call, if it needed approval (issue 419): the users
    /// whose answers made the decision, each with the rule that answered
    /// for them if one did, and when it was decided. For the record, as
    /// `answers`; never given to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
}

pub struct Approval {
    pub decided_by: Vec<(ActorId, String, Option<ApprovalRuleId>)>,
    pub at: DateTime<Utc>,
}
```

The result is part of the tool message, which goes into the transcript and the history as every
message does, published by the call's visibility (`ToolVisibility`): where the result is the
owner's, the approval is withheld with it. Nothing new is stored anywhere else; the chat shows it
where it shows the answers.

### How it gets there

The approver knows who answered; the agent does not. The decision it sends carries it:
`ApprovalResponse` gains `decided_by` - the users whose answers made the decision under the
strategy (one for `Any`, all for `All`), each with their `by_rule` - set by the approver alone, as
`by_rule` is set by a user's actor alone. `Turn::decide` (`tool_calls.rs`) keeps it with the
call's `Ready` status, as it keeps the answers, and `ToolCallResult::of` takes it over when the
call ran. A call that was denied or timed out gets no result of a run; its `status` says so, and
the approval is kept there too, so that "denied by" shows as "approved by" does.

### Not in the history as an entry

[417](417-user-history-from-its-actor.md) records in the channel's history what entered it, with
its recipients. Approvals are not among the entries: they are the state of a call, restored from
the agent's turn on a reload, and their round trip through the approver is plumbing. The fact for
the reader is here, at the result; the act for the audit is in 140.

## Done when

- `ToolCallResult` and the `Ready` status carry who approved - or denied - the call, by which rule
  and when; `ApprovalResponse` carries it from the approver.
- The chat shows "approved by …" and "denied by …" at the call, with the rule where one answered.
- A test in the agents' security suite: a call approved by one of two users records that user, and
  one answered by a rule records the rule.
- TOOLS.md and APPROVALS.md say what is recorded of an approval and where.

## History

- 2026-10-17: Opened while deciding whether the channel's history is the basis of an audit
  ([417](417-user-history-from-its-actor.md), [140](140-audit-log.md)): it is not, and approvals
  are not recorded there; what the reader of a message needs of an approval is kept at the call's
  result, as the answers to its questions are.
