# 367 — The prompt facets live in `context.rs`, their components among the components of no facet

**Status:** done
**Area:** Code structure
**Epic:** [349](349-core-review-epic.md)
**Summary:** The prompt facets live in a flat `context.rs` and their components among the components of no facet in `component_impl/`, unlike every other facet with a module of its own.

## The problem

Tools, commands, language models, moderators and compaction each have a module of their own: its
`mod.rs` holds the facet trait and `register_all`, its other files are the components that provide
the facet (COMPONENTS.md, the table of modules). What an agent's model is given does not:

- the facets - `PromptProvider`, `ContextProvider` - and what their components share - `Prompt`,
  `PromptPosition`, `Section`, `titled`, `render_sections`, `ContextError` - are a flat
  `caelumora-runtime/src/context.rs` (232 lines);
- the components that provide them alone are files of `component_impl/`, which is meant for
  "everything else, and every component that offers several facets":
  `channel_briefing_prompt_provider`, `channel_language_prompt_provider`,
  `channel_members_prompt_provider`, `static_system_prompt_provider`, `tool_sets_prompt_provider`,
  `workspace_prompt_provider`, `turn_context_provider`, `external_context_provider`; and
  `component_impl::register_all` registers the `ContextProvider` as a single with its default.

## Why it matters

Whoever looks for what goes into the model's context finds the facets in one place and their
components among notes, memory and the plan tool, while every other facet is found with its
components. `component_impl/` is larger than it has to be, and the rule COMPONENTS.md states is not
kept.

## Considerations

- **A module `prompt/`**, built as the other facet modules are:
  - `mod.rs`: the content of `context.rs` - the two facets, what their components share - and
    `register_all`, which registers the single `ContextProvider` with its default
    (`turn_context_provider`) and the kinds of the module;
  - one file per component kind, named after its kind as everywhere: the eight above, moved from
    `component_impl/` (`git mv`).
- **What stays in `component_impl/`:** the components that offer a prompt part beside other facets -
  `agent_memory`, `notes`, `private_notes`, `create_plan_tool` - and `channel_history`; by the rule
  of COMPONENTS.md, a component of several facets cannot be sorted by one.
- **The name.** `prompt` for both facets, though the `ContextProvider` assembles the whole context:
  what it assembles are the prompt parts, and both facets are about what the model is prompted
  with. `context.rs` goes; no re-export under the old path is kept. The paths change from
  `crate::context::…` (about 30 files in the runtime) and `caelumora_runtime::context::…` (the
  plugins `caelumora-a2a`, `caelumora-ollama`, `caelumora-skills`) to `…::prompt::…`.
- **Unchanged:** the kind constants (`STATIC_SYSTEM_PROMPT_PROVIDER_KIND` and so on), which are
  what configurations and stored personalities name. No stored data and no configuration changes.
- COMPONENTS.md gets the module in its table; AGENT-TURN.md and the documents that name
  `crate::context` follow.
- Moves only, no behaviour changes.

## Done when

- `prompt/` holds the two facets, what they share, `register_all` and the eight components;
  `context.rs` is gone, and `component_impl/` holds only components of several facets or of no
  facet module.
- No behaviour changes; clippy and the tests pass.

## History

- **2026-10-15** — Opened at the maintainer's request, under epic [349](349-core-review-epic.md),
  with the other moves of files ([364](364-managers-in-different-places.md),
  [366](366-runtime-rs-holds-four-types.md), [368](368-component-module.md)).
- **2026-10-15** — Done, as considered; no behaviour changes.
  - `context.rs` went to `prompt/mod.rs` (`git mv`), with a description of the module in place of
    that of the two facets, the eight `pub mod`s and `register_all`, which registers the eight
    kinds and the single `ContextProvider` with its default; `ComponentRegistry::built_in` calls it
    after `component_impl`'s.
  - The eight components went from `component_impl/` to `prompt/` (`git mv`), unchanged but for
    their paths. `component_impl/` keeps `agent_memory`, `channel_history`, `create_plan_tool`,
    `notes` and `private_notes`; its description names `prompt` among the facets' modules.
  - `crate::context::…` is `crate::prompt::…` in the runtime, and `caelumora_runtime::context::…`
    `caelumora_runtime::prompt::…` in `caelumora-skills`; the other plugins named only
    `caelumora_model::context`, which stays. Where tests wrote the old path out
    (`channel/tests`, `turn_context_provider`), the types are imported now.
  - COMPONENTS.md has `prompt/` in its table of modules and `crate::prompt` in that of the facets;
    PLUGINS.md and AGENT-TURN.md name `prompt::ContextError`, `prompt::titled`.
