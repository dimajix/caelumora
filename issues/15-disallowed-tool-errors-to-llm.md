# 15 — Return errors to the LLM when a tool is disallowed

**Status:** done
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
- **2026-09-22** — Implemented. Found: every path already produced a text
  result (`Tool execution denied`, `Tool execution timed out`,
  `Error: permission denied: <tool>`, `Error: <tool error>`), so nothing was
  silently dropped - but nothing told the model whether to retry, the policy
  was only checked *after* a user had approved, and neither the UI nor a
  record could tell a refusal from a result. Decisions:
  - **Format:** every outcome other than success is a JSON object
    `{"status": ..., "message": ...}`; successful results are unchanged.
  - **Statuses and wording**, each with a next step: `denied` ("Do not
    repeat it; continue without it or ask the user"), `forbidden` ("not
    permitted. Do not retry it" - no reason; the reason is logged),
    `timed_out` ("You may ask the user whether to try again"),
    `invalid_arguments` (with the reason, "correct them and call again"),
    `failed` (with the tool's error), `unknown_tool` (with the list of
    available tools). In `crate::tool`: `ToolError::to_result`,
    `denied_result`, `timed_out_result`, `forbidden_result`.
  - **Policy before approval:** `ToolExecutor::is_permitted` is checked in
    `request_approvals`; a forbidden call gets its result immediately and no
    approval request. `execute` still checks.
  - **Status stored:** `ToolResultStatus` in `ToolCallResult::status`
    (omitted when `ok`, so existing histories read unchanged) and
    `ToolCallState::result_status` (defaulted, so stored agent states still
    load). The web UI marks unsuccessful calls with their status and shows
    the message instead of the raw JSON.
  - **Unavailable tools** (an `mcp_tool` whose server was unreachable at
    build time) split off into [32](32-unavailable-tools-invisible-to-model.md):
    there is no call to answer, so it needs a way into the model's context,
    which is a separate decision.
  - Verified against the REST server: asked to call its `silence` tool,
    Sassy requested approval; after a denial via the REST API the history
    held the tool result `{"status":"denied", ...}`, and the model answered
    the user instead of retrying. The `forbidden` path is covered by unit
    tests only - `DummySecurityManager` permits everything.

