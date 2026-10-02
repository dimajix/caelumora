# 13 — Approval workflow: "Always approve" and "Brave mode"

**Status:** open
**Area:** Tools & approvals

## The problem

Approval today is per tool call and per decision. `ApproverActor` collects responses from the human
participants in a channel and resolves them according to the `Approver`'s `ApprovalStrategy` (`Any`,
`Majority`, `All`, `DenyOnFirst`). What it cannot do is remember a decision.

There is no way to say "yes, and stop asking me about this tool", and no way to say "I trust this
session, run everything". Every call to a benign, repeatedly-used tool costs a round trip through a
human.

## Why it matters

Approval fatigue is a security problem, not a convenience problem. A prompt that appears ten times
for `calculator_tool` trains the user to approve without reading, which is exactly the state in
which the eleventh prompt — the one that matters — gets approved too.

## Considerations

- The two modes are different in kind and should not be built as one switch:
  - **Always approve** is a *scoped, remembered decision*: this tool (possibly with these
    arguments), for this agent, in this session or channel, until revoked. It needs a scope model
    and somewhere to persist it.
  - **Brave mode** is a *blanket suspension* of approval for a session. It is trivially
    implementable and correspondingly easy to leave on by accident. It needs to be visible while
    active and should probably expire.
- Scoping "always approve" by tool *name* alone is too coarse — a shell tool approved once is
  approved for every command. Scoping by name plus argument shape is more defensible; deciding how
  far to go is the substance of this issue.
- A remembered approval is a grant, and grants belong in the audit record
  ([05](05-postgres-event-recorder.md)) and under the same access rules as giving the approval in
  the first place ([14](14-approval-privilege.md)).
- Interacts with [15](15-disallowed-tool-errors-to-llm.md): a denial and a never-asked are different
  outcomes and the agent should be able to tell them apart.

## Done when

- A remembered approval can be granted with an explicit scope and revoked.
- Brave mode, if implemented, is visible for as long as it is active and does not outlive its
  session.

## History

- **2026-09-10** — Issue created from `TODO.md`.
