# 118 — A tool call with invalid arguments is put up for approval before it fails

**Status:** open
**Area:** Tools & approvals

## The problem

When the model calls a tool that requires approval, `request_approvals`
(`caelumora-runtime/src/actor_impl/agent.rs:846`) asks the executor to
`describe` the call for the approval prompt:

```rust
let description = self.tool_executor.describe(tc).unwrap_or("No description".to_string());
```

A `describe` that fails because the arguments are invalid - e.g. a
`create_plan` call that is not a plan, which `parse_plan` now reports as
`ToolError::InvalidParameter` (issue [82](82-tool-parameter-parsing.md)) -
is turned into "No description", and the call is put up for approval all
the same. Only after a user approved it does `execute` fail with the same
`InvalidParameter`, and the model is asked to correct its arguments.

## Why it matters

- A user is asked to approve something that cannot run, described as "No
  description" - the approval prompt says nothing about what would happen.
  Approving without reading is exactly what issue
  [13](13-approval-modes.md) wants to avoid training.
- The model's correction waits for a human for no reason: the runtime knew
  before asking that the call would fail.
- Other `describe` failures are swallowed the same way (`UnknownTool` for a
  tool the model invented), with the same effect.

## Considerations

- **Answer the call right away.** In `request_approvals`, a `describe`
  that fails with `InvalidParameter` or `UnknownTool` gives the call its
  result immediately (`ToolError::to_result`, the same text `execute` would
  produce) instead of a pending approval; the call never reaches a user.
  Calls of the same batch that are fine go on as today.
- **Validate before approval, separately.** A `ToolProvider::validate`
  facet next to `describe`. More API for what `describe` already finds out;
  not recommended.
- Other `describe` errors (a provider that fails for another reason) keep
  today's behaviour, with the error logged rather than hidden behind "No
  description".

## Done when

- A call whose arguments are invalid, or whose tool does not exist, is
  answered to the model without asking anyone.
- The approval prompt never says "No description" because describing
  failed.

## History

- **2026-09-29** — Noticed while fixing [82](82-tool-parameter-parsing.md);
  created at the maintainer's request. Part of the review epic
  [74](74-refactoring-epic.md).
