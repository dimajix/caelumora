# 52 — All built-in tools live in one file

**Status:** open
**Area:** Tools & approvals

## The problem

`caelumora-runtime/src/component_impl/tool.rs` holds every built-in tool
provider in one file of about 500 lines: `echo_tool`, `calculator_tool`,
`stop_tool` and `create_plan_tool` (with its planning prompt and plan
parsing), their shared registration, and all their tests. Tools that came
later went elsewhere - `mcp_tool` in `component_impl/mcp.rs`, the tools of
`notes` in `component_impl/notes.rs` - so there is no single place where
tools are, and no rule for where a new one goes.

## Why it matters

- Every new tool makes the file longer, and changes to one tool touch a file
  shared by all.
- The runtime already organises implementations of a trait as `*_impl`
  directories with one file per implementation (`actor_impl`, `llm_impl`,
  `moderator_impl`, `transport_impl`). Tools do not follow that pattern.

## Considerations

- A new directory `caelumora-runtime/src/tool_impl/`, next to the other
  `*_impl` directories and to `tool.rs` (the `ToolProvider` trait and the
  executor), with one file per tool provider: `echo.rs`, `calculator.rs`,
  `stop.rs`, `create_plan.rs`, each with its kind constant, its tests and its
  registration; `tool_impl/mod.rs` registers them all.
- Which components count as tools: `mcp_tool` is a tool provider and
  belongs there too. `notes` and `channel_history` (issue
  [17](17-information-sharing-within-session.md)) provide tools but are also
  prompt providers; whether they stay in `component_impl` or move is to be
  decided, the same question as for `create_plan_tool`, which provides a
  prompt part as well.
- The kind strings (`echo_tool`, ...) stay: stored personalities refer to
  them. The Rust paths change; `caelumora-rest-server/src/data.rs` and
  `caelumora-tui/src/main.rs` import the kind constants and must follow.
- A pure move, no change in behaviour.

## Done when

- Every built-in tool provider is in its own file under `tool_impl/`, with
  its tests; `component_impl/tool.rs` is gone.
- It is recorded in `ARCHITECTURE.md` where a new tool goes.

## History

- **2026-09-25** — Issue created at the maintainer's request, while tools
  for reading notes and histories were being added in
  [17](17-information-sharing-within-session.md).
