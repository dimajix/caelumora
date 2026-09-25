# 53 — Facet traits live in different places, and one name is used twice

**Status:** done
**Area:** Maintenance

## The problem

The traits components implement as facets are spread over the runtime crate
without a rule:

- `ToolProvider` is in `crate::tool`, `CommandProvider` in `crate::command` -
  next to the executors that use them.
- `PromptProvider`, `ContextProvider` and `Prompt` are in
  `component_impl::context`, and `MessageProcessor`, `PerceiveInProcessor`
  and `EmitOutProcessor` in `component_impl::message` - in the directory that
  otherwise holds component implementations only (issue
  [52](52-split-tool-providers.md)).
- `crate::message` has a second, unrelated `MessageProcessor` trait
  (actor-like: `id`, `kind`, `process_messages` returning a stream), so the
  same name means two different things depending on the import.

## Why it matters

- Whoever writes a component has to know where each facet trait happens to
  live.
- Two traits named `MessageProcessor` invite importing the wrong one and make
  every mention in docs ambiguous (`ARCHITECTURE.md` lists `MessageProcessor`
  among the component traits).

## Considerations

- The pattern that exists: a facet trait next to the code that consumes it -
  `crate::tool` (trait and `ToolExecutor`), `crate::command` (trait and
  `CommandExecutor`). Following it would move `PromptProvider`,
  `ContextProvider` and `Prompt` to a module such as `crate::context`, and
  the stage processor traits to a module of their own; `component_impl`
  would hold implementations only.
- The two `MessageProcessor`s: find out whether `crate::message::
  MessageProcessor` and `MessageFilter` are still used. If not, remove them;
  if they are, rename one of the two.
- A pure refactor: paths change across runtime, REST server and TUI, no
  behaviour does.

## Done when

- Every facet trait lives next to the code that consumes it, and
  `component_impl` holds component implementations only.
- No two traits share a name.
- `ARCHITECTURE.md` says where facet traits go.

## History

- **2026-09-25** — Issue created while splitting `component_impl` into one
  file per component ([52](52-split-tool-providers.md)).
- **2026-09-25** — Done:
  - The second `MessageProcessor` was dead code: `crate::message` held it
    and `MessageFilter`, which nothing used; the trait's only user was
    `actor_impl/message_processor.rs` with a `MessageProcessorActor` that
    nothing created. Both files were deleted (with the maintainer's
    agreement), which also removed two build warnings from that actor.
  - `component_impl::message` moved to `crate::message` (`MessageProcessor`,
    `PerceiveInProcessor`, `EmitOutProcessor`), `component_impl::context` to
    `crate::context` (`ContextProvider`, `PromptProvider`, `Prompt`,
    `PromptPosition`) - next to `crate::tool` and `crate::command`.
    `component_impl` holds components only.
  - `ARCHITECTURE.md` lists the actual facets and says where facet traits
    go; it had named a `PlanPreProcessor` that does not exist.
  - No change in behaviour; all tests pass.
