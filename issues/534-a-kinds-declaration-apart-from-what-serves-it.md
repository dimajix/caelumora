# 534 — A component kind's declaration lies in one file with the type that serves its facets

**Status:** in progress
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** In the crates that bring a component kind, `component.rs` holds the kind's declaration - its parameters, its facets and needs, how it is built - in one file with the type that serves its facets and all its behaviour, so that what a kind is configured with is found only by reading through what its tools do, and that type is named `…Component` though it is no component but what serves one.

## The problem

A crate of its own that brings a component kind keeps it in `component.rs`
([COMPONENTS.md](../docs/COMPONENTS.md#writing-a-component)). There, in one file, lie two things:

- **The kind's declaration:** the kind's constant, its `Params`, the `ComponentKind` with what it
  `provides`, `reads`, `writes` and `needs`, and the factory in `register` that reads the
  parameters and builds the type. A reader who wants to know how a kind is configured and what it
  depends on wants this.
- **The type that serves the facets**, with all its behaviour: the tools' arguments, their
  execution, their texts, the prompt part, the commands - and the tests of it.

The second is most of the file: `caelumora-tool-workspace` 763 lines, `caelumora-skills` 675,
`caelumora-tool-attachment` 647, `caelumora-tool-mcp` 563, `caelumora-tool-image` 414,
`caelumora-compaction` 379, `caelumora-artifacts` 236. The declaration is spread through it - the
`Params` near the top, the `ComponentKind` at the bottom.

And the type is named after the component, not after what it is: `AttachmentsComponent`,
`ImageGenerationComponent`, `McpServerComponent`, `ArtifactsComponent`, `SkillsComponent`,
`WorkspaceComponent`, `CompactionComponent`. It is what serves the component's facets - a
`ToolProvider`, a `PromptProvider` -; the component is the whole the kernel builds and holds
(`BuiltComponent`).

Half the rule exists already: "a struct that is what a facet asks for lies in a module named after
that role" - `policy.rs` of `caelumora-decision` with `DecisionPolicy`, `summary_compaction.rs` of
`caelumora-compaction` with `SummaryCompaction`, `agent/` of `caelumora-a2a` with `A2aAgent`. Those
types are named after their role; but their modules hold their kinds' declarations as well, so the
rule separates by role only where the role is not that of `component.rs`.

Proposed by the maintainer on 2026-11-03, in the discussion of issue
[533](533-attachment-tools-in-a-crate-of-their-own.md), as a pattern for every library.

## Considerations

- **The cut.** `component.rs` declares and builds the crate's kinds: the constants, the `Params`,
  `register` with the `ComponentKind` and the factory. The type that serves the facets lies in a
  module of its own named after its role, with its tests - `tool.rs` for a type that is a
  `ToolProvider` alone. `component.rs` becomes short where a kind has no parameters
  (`attachments`: its constant and a `register` of some fifteen lines); the cut pays most where
  the parameters are many and checked (`mcp_server`, `workspace`, `image_generation`, `skills`).
  The rule is for uniformity too: a reader of any of these crates finds the configuration in the
  same place.
- **Types of several facets.** `SkillsComponent` serves tools, a prompt part, commands and a
  `KnowledgeRestorer`; `CompactionComponent` commands and an activity; `WorkspaceComponent` tools
  and a prompt part, deliberately one kind since issue
  [532](532-component-kinds-named-after-their-facet.md). A `tool.rs` would hold one of their
  facets. Their module is named after what the type is, not after one facet - which name, is to be
  decided for each. Spreading the `impl`s of one type over a file per facet was considered and is
  not proposed: one type would then be read in several places.
- **The names of the types.** Each `…Component` is renamed after what it is - say
  `AttachmentTools`, `ImageGeneration`, and for each of the others as its plan proposes -, as
  `SummaryCompaction`, `DecisionPolicy` and `A2aAgent` are named already.
- **The modules that are named after their role already** - `summary_compaction.rs`, `policy.rs`,
  `agent/mod.rs` - follow the same cut: their kinds' declarations move to `component.rs`, which in a
  crate of several kinds (`caelumora-compaction`) declares all of them. `caelumora-decision` is a
  plugin and registers through the `ComponentRegistry`; the rule is the same.
- **The built-in kinds of `caelumora-agents`** keep one file per kind: there the module already
  sorts them by facet (`tool/`, `prompt/`, `components/`), and most are small. COMPONENTS.md
  distinguishes the two already, and keeps doing so.
- **Visibility, in the same go.** The split makes the type and its constructor `pub(crate)`, since
  `component.rs` builds it; everything else stays as private as it can. The review of issue 533
  found two crates that make more visible than they use, which this touches anyway:
  - `caelumora-tool-image/src/component.rs`: `IMAGE_GENERATION_KIND` and `GENERATE_IMAGE` are
    `pub(crate)`, used only in their own module.
  - `caelumora-tool-workspace`: its module `component`, `NAMESPACE`, `PROMPTS`,
    `WorkspaceComponent`, `register` and the tools' names are public; outside the crate only
    `WORKSPACE_KIND` is used, by `tests/workspace_tools.rs`. Aligned with `caelumora-tool-image`:
    `mod component` private, the kind's name alone reachable for the integration test.

  The constants and types of `caelumora-skills` and `caelumora-artifacts`, which are a plugin and
  a library at once, are checked the same way.
- **Steps.** A refactoring without a change of behaviour, one step per crate, each reviewed on its
  own: first the rule in COMPONENTS.md with `caelumora-tool-attachment` and
  `caelumora-tool-image`, the two smallest; then the others. The names of the types and of the
  modules of the types of several facets are put before the maintainer with the plan.

## Done when

- In every crate that brings a component kind, `component.rs` holds its kinds' declarations and
  nothing else, and the types that serve their facets lie in modules named after their roles,
  named after what they are.
- No such crate makes visible more than it uses from outside.
- `docs/COMPONENTS.md` states the rule, and the documents that name a renamed type name it so.

## History

- **2026-11-03** — Opened at the maintainer's request, from the discussion of issue
  [533](533-attachment-tools-in-a-crate-of-their-own.md).
- **2026-11-03** — Decided with the maintainer, in place of the cut and the renaming the
  considerations propose: the struct that takes the configuration in its `new` keeps its name
  `…Component`, as issue [532](532-component-kinds-named-after-their-facet.md) decided; what moves
  out of `component.rs` is the `impl` of each facet it serves. The module `component` becomes a
  directory: its `mod.rs` holds the kind's constant, its `…Params`, the `…Component` with `new`, and
  `register`; a child module per facet - `tool.rs`, `prompt.rs`, … - holds that facet's `impl`,
  what only it uses, and its tests, a child so that it reaches the private fields of the
  `…Component`. Set aside: a type of its own per facet without any dependency on the components -
  its configuration would have had to be passed apart from the `…Params`, and nothing uses such a
  type outside a component. Taken from it: `new` fails with the error of what it does itself,
  never with a `ComponentError`; whether the configuration holds is said by `…Params::read` and
  `check` in `register`. The types already named after their role - `SummaryCompaction`,
  `DecisionPolicy`, `A2aAgent` - are no contributing component and stay as they are. One step per
  crate, the first with the rule in COMPONENTS.md.
- **2026-11-03** — Step 1: the rule in COMPONENTS.md - the layout of the module `component`, and
  the error rule in its point **Construction** -; `caelumora-tool-attachment` and
  `caelumora-tool-image` laid out so. `ImageGenerationComponent::new` refused an agent without an
  image model with a `ComponentError`; decided with the maintainer, `register` refuses it now,
  where the graph is asked for the models, and `new` takes them as not empty. Its test builds the
  component through a kernel of a test, with `caelumora-kernel` and `caelumora-security` as
  dev-dependencies of the crate.
