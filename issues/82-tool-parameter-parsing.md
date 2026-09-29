# 82 — `create_plan` reports parse errors with the wrong status

**Status:** done
**Area:** Tools & approvals

## The problem

`create_plan_tool.rs:116,142` map a parse failure of the tool's arguments
to `ToolError::ExecutionFailed` (status `Failed`, "The tool failed"). The
model is meant to see `InvalidParameter` (status `InvalidArguments`,
"Correct them and call the tool again", `tool.rs:92-95`), so that it
corrects the call instead of giving up (issue
[15](15-disallowed-tool-errors-to-llm.md)).

## Why it matters

The model reacts to the status. `Failed` says the tool is broken; the
model will not retry with corrected arguments.

## Considerations

Map `serde_json::from_value` errors to `InvalidParameter` with the serde
message. Two lines and a test. How every tool should read its parameters is
issue [111](111-tool-parameter-reading.md); this fix does not wait for it.

## Done when

A malformed `create_plan` call is answered with `InvalidArguments`.

## History

- **2026-09-28** — Found in the structure review
  (`work/2026-09-28-code-review.md`, section 1, error 8). The unification of
  parameter reading was split off into 111 at the maintainer's request.
- **2026-09-29** — Done. `parse_plan` returns `ToolResult<Plan>` and maps
  a serde error to `ToolError::InvalidParameter("not a plan: …")`, so
  `execute` and `describe` use it with `?`. The tests assert the variant
  and that serde names the missing field. Noticed on the way: a malformed
  call is still put up for approval first - `request_approvals` turns the
  failed `describe` into "No description" (`agent.rs:846`), and only the
  approved call then fails with `InvalidArguments`. The user approves
  something that cannot run; the call should be answered right away
  instead. Not part of this issue.
