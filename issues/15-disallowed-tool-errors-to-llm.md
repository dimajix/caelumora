# 15 — Return errors to the LLM when a tool is disallowed

**Status:** open
**Area:** Tools & approvals

## The problem

When a tool call is refused — denied by a user, blocked by the
`SecurityManager` on `ActionType::CallTool`, or timed out waiting for an
approver — the agent's cognitive cycle handles it, but the model is not
reliably told *what* happened in a form it can act on.

An agent that gets silence, or an opaque failure, where it expected a tool
result tends to do one of two unhelpful things: retry the same call, or
proceed as though the call had succeeded.

## Why it matters

A refusal is information. "You may not call this tool" lets the model pick a
different approach or tell the user why it cannot continue. Anything vaguer
turns a clean policy decision into an agent that appears broken — and, in the
retry case, into repeated approval prompts, which feeds
[13](13-approval-modes.md).

## Considerations

- The distinct outcomes should stay distinct in what the model sees: denied by
  a human, forbidden by policy, timed out, and tool-failed-while-running are
  four different situations calling for four different next steps.
- There is a disclosure tension. Telling the model precisely why a policy
  denied a call describes the policy to something that will happily try to
  route around it. "Not permitted" without the reasoning is the safer default;
  the reason belongs in the log and the audit record.
- The same mechanism should cover a tool that is not available at all — e.g.
  an `mcp_tool` whose server was unreachable at build time, which currently
  just means the tool silently does not exist.
- Related: [02](02-better-error-messages.md) covers the human-facing half of
  the same weakness.

## Done when

- Every refusal path feeds a structured result back into the agent loop rather
  than a bare failure.
- Denial, policy refusal and timeout are distinguishable to the model, without
  disclosing policy internals.

## History

- **2026-09-10** — Issue created from `TODO.md`.
