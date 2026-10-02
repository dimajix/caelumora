# 111 — Tools read their parameters in four different styles

**Status:** done
**Area:** Tools & approvals

## The problem

Every built-in tool receives `params: &Value` and reads it its own way:

- `params.get("text").and_then(Value::as_str)` with a hand-written `InvalidParameter` (`notes.rs`,
  `private_notes.rs`, `channel_history.rs`);
- `params["expression"].as_str()` (`calculator_tool.rs:34`);
- a serde struct via `serde_json::from_value` (`create_plan_tool.rs:55-67`);
- passing the value through untouched (`echo_tool.rs`, `mcp_tool.rs`).

`note_id` (`notes.rs:163`) is copied in `agent_memory.rs:93-96` for `MemoryId`, and `note_text` is
imported from `notes` by the other two components although it is a general argument helper.

## Why it matters

Four styles are four places to get the error status wrong (issue [82](82-tool-parameter-parsing.md)
is the one that was). A new tool copies the nearest example and adds a fifth. The parameter schema
(`ToolParameters`) and the reading code are written twice today and can disagree.

## Considerations

- **serde structs everywhere**, `serde_json::from_value(params.clone()) .map_err(|e|
  ToolError::InvalidParameter(e.to_string()))`. Documents the parameters in one place, matches the
  schema, gives the model serde's message about what was wrong. About eight tools.
- **A small `tool::args` module** (`required_str`, `optional_str`, `required_u32`, `id_param<T:
  From<u32>>`), for tools with one or two parameters where a struct feels heavy. Fewer types, but
  the schema stays separate from the reading code.
- Either way, one helper that turns a parse error into `InvalidParameter`, so that the status is
  right by construction.
- Pass-through tools (echo, MCP) stay as they are; the MCP server validates its own schema.

Recommendation: serde structs, with `id_param` as the one shared helper for numbered entries. Decide
before the next tool is written.

## Done when

- Every built-in tool that reads parameters does it the same way.
- A malformed call to any of them is answered with `InvalidArguments`.

## History

- **2026-09-28** — Split off from [82](82-tool-parameter-parsing.md) at the maintainer's request;
  from the structure review (`work/2026-09-28-code-review.md`, 4.2.4).
- **2026-09-29** — Done as recommended. `crate::tool::arguments::<T>(params)` reads a call's
  arguments as a struct that names the tool's parameters and turns a mismatch into
  `InvalidParameter` with serde's message; no arguments at all read as an empty object. Every
  built-in tool that reads parameters uses it: `note`, `note_private`, `remember` (through
  `note_text`), `revoke_note`, `revoke_private_note`, `forget` (through the generic
  `IdArgument<T>` - `NoteId` and `MemoryId` deserialise themselves, so no `id_param` helper was
  needed), `read_notes`, `read_history`, `calculate` and `create_plan`. Checks that are not about
  the shape - an empty text, an empty channel name - stay with the tool. Echo and MCP still pass
  their arguments through. What the model is told changed where serde now words it: "missing field
  `text`" instead of "'text' must not be empty" for a missing field, and a `last` that is not a
  whole number is refused instead of silently replaced by the default. A test covers the helper.
