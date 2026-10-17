# 52 — All built-in tools live in one file

**Status:** done
**Area:** Tools and approvals
**Summary:** All built-in tool providers live in one file, and tools added later went to other places, so there is no rule for where a tool goes.

## The problem

`caelumora-runtime/src/component_impl/tool.rs` holds every built-in tool provider in one file of
about 500 lines: `echo_tool`, `calculator_tool`, `stop_tool` and `create_plan_tool` (with its
planning prompt and plan parsing), their shared registration, and all their tests. Tools that came
later went elsewhere - `mcp_tool` in `component_impl/mcp.rs`, the tools of `notes` in
`component_impl/notes.rs` - so there is no single place where tools are, and no rule for where a new
one goes.

## Why it matters

- Every new tool makes the file longer, and changes to one tool touch a file shared by all.
- The runtime already organises implementations of a trait as `*_impl` directories with one file per
  implementation (`actor_impl`, `llm_impl`, `moderator_impl`, `transport_impl`). Tools do not follow
  that pattern.

## Considerations

- A new directory `caelumora-runtime/src/tool_impl/`, next to the other `*_impl` directories and to
  `tool.rs` (the `ToolProvider` trait and the executor), with one file per tool provider: `echo.rs`,
  `calculator.rs`, `stop.rs`, `create_plan.rs`, each with its kind constant, its tests and its
  registration; `tool_impl/mod.rs` registers them all.
- Which components count as tools: `mcp_tool` is a tool provider and belongs there too. `notes` and
  `channel_history` (issue [17](17-information-sharing-within-session.md)) provide tools but are
  also prompt providers; whether they stay in `component_impl` or move is to be decided, the same
  question as for `create_plan_tool`, which provides a prompt part as well.
- The kind strings (`echo_tool`, ...) stay: stored personalities refer to them. The Rust paths
  change; `caelumora-rest-server/src/data.rs` and `caelumora-tui/src/main.rs` import the kind
  constants and must follow.
- A pure move, no change in behaviour.

## Done when

- Every built-in tool provider is in its own file under `tool_impl/`, with its tests;
  `component_impl/tool.rs` is gone.
- It is recorded in `ARCHITECTURE.md` where a new tool goes.

## History

- **2026-09-25** — Issue created at the maintainer's request, while tools for reading notes and
  histories were being added in [17](17-information-sharing-within-session.md).
- **2026-09-25** — Decided with the maintainer before implementing: **no `tool_impl` directory**. A
  component may offer several facets (`create_plan_tool` and `notes` provide tools and a prompt
  part), so sorting by facet does not work; instead **every component gets a file of its own in
  `component_impl`, named after its kind**. That applies to all components, not only tools. Done:
  - `tool.rs` split into `echo_tool.rs`, `calculator_tool.rs`, `stop_tool.rs` and
    `create_plan_tool.rs`; `command.rs` into `help_command.rs` and `wakeup_command.rs`; the
    components of `context.rs` into `state_context_appender.rs`, `state_context_provider.rs` and
    `static_system_prompt_provider.rs`; `history.rs` renamed to `state_history_writer.rs`, `mcp.rs`
    to `mcp_tool.rs`.
  - Each file has its kind constant, `register` and its tests; `component_impl::register_all`
    replaces the list in `RuntimeManager::new`.
  - `context.rs` and `message.rs` now hold only facet traits. Where facet traits belong is uneven
    across the crate and was opened as [53](53-facet-trait-placement.md) rather than changed here,
    to keep this a pure move.
  - No change in behaviour: kind strings unchanged, all 98 runtime tests pass as before.
- **2026-09-25** — Named consistently with `state_context_appender`: `state_history_writer` is now
  `state_history_appender` (file, kind string, `StateHistoryAppender`,
  `STATE_HISTORY_APPENDER_KIND`). Unlike the other kinds, this kind string changed, so stored
  personalities that refer to `state_history_writer` no longer load (legacy data need not be
  supported).
- **2026-10-01** — Revised with the maintainer, outside of an issue: the components whose purpose is
  one facet moved to directories of that facet - `tool_impl/` (`echo_tool`, `calculator_tool`,
  `sleep_tool`, `stop_tool`, `mcp_tool`, `default_tool_executor`), `command_impl/` (`help_command`,
  `wakeup_command`) and `llm_impl/` (`ollama_client`), next to `moderator_impl/`. The reason against
  `tool_impl` above still holds for components with several facets: `create_plan_tool`, `notes`,
  `private_notes`, `agent_memory` and `channel_history` stay in `component_impl`, as do the context
  and message components. Each directory has its own `register_all`; a pure move, kind strings
  unchanged.
