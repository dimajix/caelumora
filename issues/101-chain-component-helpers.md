# 101 — The three chain components repeat their helpers, and `AgentNotes`/`AgentMemory` are structs for no reason

**Status:** done
**Area:** Context & knowledge

## The problem

`notes`, `private_notes` and `agent_memory` have the same shape: two tools
(add text, remove by number), one `BeforeTurnInput` part with `- #id: text`
lines under a heading, one `KnowledgeRef` per chain. The review concluded
that a generic component does *not* pay off (the differing parts -
`read_notes`, other agents' notes, "elsewhere" - dominate; the generic part
belongs in the model, issue [92](92-ledger-for-notes-and-memory.md)). But
the helpers are copied: `note_id` (`notes.rs:163`) in `agent_memory.rs:93-96`;
`note_text` imported from `notes` by the other two; three `render`
functions (`notes.rs:218`, `private_notes.rs:143`, `agent_memory.rs:125`);
`read_error` (`notes.rs:172-177`) as a closure in `channel_history.rs:68-71`.

`AgentNotes` and `AgentMemory` are constructed, called once and dropped at
every use (`AgentNotes::new(turn, kind).note(...)`, six sites); their read
functions are already associated functions taking `&Turn`.

Names differ without reason: kind constants `*_TOOL_PROVIDER_KIND` vs.
`*_KIND`, command kinds in the model and all others in the runtime; struct
names `NotesComponent` vs. `ChannelHistoryReader` vs. `CreatePlanToolProvider`;
tool names as constants or as doubled literals (`calculator_tool.rs:21,33`,
`echo_tool.rs:20,32`, `stop_tool.rs:26-51`); `facets` vs. `capabilities`;
`::new(reader)` vs. struct literal. After the rename context → turn
(`f5dcaff`), parameters are still `context: &Turn` in `notes.rs:124,191`,
`private_notes.rs:122`, `agent_memory.rs:92,111`, `calculator_tool.rs:32`,
`echo_tool.rs:31`, `agent.rs:517`.

## Why it matters

A component author copies the nearest example; today the nearest example
differs from the next one.

## Considerations

- `id_param<T: From<u32>>`, `note_text` and `render_sections(preamble,
  sections)` in `tool.rs`/a shared module; `NotReadable::to_tool_error` in
  `session_reader.rs`.
- `AgentNotes::note(turn, kind, text)` etc. as associated functions, or
  `NoteKind` as the receiver. The different id assignment (notes: chain +
  staging; memory: `reserve_memory_id` on the shared handle, because several
  actors of one agent write concurrently) is deliberate and should be stated
  in `agent_memory.rs` (issue [19](19-memory-tiers.md) records it as the
  exception to "commit, don't write").
- One naming convention for kinds, structs, tool names and the turn
  parameter; ten mechanical edits.

## Done when

- No helper is defined twice among the components.
- Kinds, struct names and tool names follow one convention.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 4.1.4, 4.2.3, 4.2.5).
- **2026-09-29** — Done, as discussed with the maintainer.
  Helpers: `tool::text_argument` and `id_argument` replace `note_text`,
  `note_id` and the inline `IdArgument` (now gone). Instead of a shared
  `read_error`, the `SessionReader` returns a typed
  `session_reader::ReadError` (`NotReadable` | `Failed`) with
  `From<ReadError> for ToolError`, so `notes` and `channel_history` pass
  it on with `?` and nothing downcasts any more. The three `render`
  functions are built on `context::render_sections(preamble, &[Section])`.
  `AgentNotes` and `AgentMemory` have only associated functions
  (`AgentNotes::note(turn, kind, text)`, `AgentMemory::forget(turn, id)`,
  ...), which matches their read functions. `AgentMemory::remember` explains
  why memory numbers are reserved on the shared handle and are not derived
  like note numbers. Naming: kind constants `<KIND>_KIND` (no
  `_PROVIDER`), `ChannelHistoryReader` → `ChannelHistoryComponent`,
  `StaticPromptProvider` → `StaticSystemPromptProvider`, tool and command
  names as constants, `facets` everywhere, struct literals instead of
  trivial `new`s, `turn` as the parameter name. The command kinds stay in
  the model on purpose, because `default_commands` names them, and a comment
  there says so. The convention is written down in ARCHITECTURE.md under
  the built-in components.
