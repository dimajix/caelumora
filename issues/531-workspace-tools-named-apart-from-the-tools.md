# 531 — The crate of the workspace's tools is named apart from the crates of tools

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** The library of the workspace's tools is named `caelumora-workspace-tools`, subject first and plural, while the crates that bring one kind of something are named by the kind first - `caelumora-model-ollama`, and `caelumora-tool-image` of issue 525 -, so that two crates of tools follow two schemes.

## The problem

Issue [525](525-image-models-are-a-tool-s-own.md) brings the tool that generates images in a
library of its own, `caelumora-tool-image`: named by what it is first, then by its subject, as
`caelumora-model-ollama` and `caelumora-model-comfyui` are named after the models they serve. The
library of the tools on the session's workspace (issue
[428](428-workspace-tools-in-a-crate-of-their-own.md)) is named the other way round,
`caelumora-workspace-tools`, with the library's name and the namespace of its texts
`workspace_tools` and its type `WorkspaceToolsLibrary`.

So whoever looks for the crates of tools finds them under two schemes, and the next crate of tools
has no pattern to follow.

Raised by the maintainer on 2026-11-02, in the discussion of issue 525.

## Considerations

- **The name.** `caelumora-tool-workspace`, as `caelumora-tool-image`: singular, as the kind of
  crate, not the count of its tools. The library's name and the namespace of its texts follow
  (`tool_workspace`), and its type (`WorkspaceToolLibrary`, or after the crate).
- **What follows.** The workspace's `Cargo.toml`, the programs' manifests and lists of libraries,
  `scripts/crates.sh`, CRATES.md (by `scripts/crate-graph.sh`), and the documents that name the
  crate or the namespace (ARCHITECTURE.md, PLUGINS.md, TOOLS.md, COMPONENTS.md, CONFIGURATION.md,
  AGENT-TURN.md). CRATES.md records the old name, as it records those of issue
  [522](522-two-crates-named-model.md).
- **Configuration.** A variant of the texts under `prompts` that overrides texts of the namespace
  `workspace_tools` has to name the new namespace.
- **The other libraries of tools** - `caelumora-mcp`, `caelumora-a2a` - are named after a protocol,
  not as a crate of one tool; whether they follow the scheme is to be decided with this one.

After 525, so that the scheme is the one that crate set.

## Done when

- The library of the workspace's tools is named after the scheme of the crates of tools, and the
  documents name it so.

## History

- **2026-11-02** — Opened at the maintainer's request, in the discussion of issue
  [525](525-image-models-are-a-tool-s-own.md).
- **2026-11-02** — Decided: a crate that brings one kind of thing for a subject that is another's,
  and nothing else, is named by that kind first - a library that brings an agent tools
  `caelumora-tool-…` -, while a plugin that brings a service of its own with its manager is named
  after its subject, though it brings tools as well (`caelumora-skills`, `caelumora-artifacts`).
  `caelumora-a2a` brings no tool but an external agent, and keeps its name; `caelumora-mcp` brings
  nothing but the tools of MCP servers, and follows the scheme. Whether the component kinds are
  named after their facet - `_tool` - became issue
  [532](532-component-kinds-named-after-their-facet.md).
- **2026-11-02** — Done. `caelumora-workspace-tools` is `caelumora-tool-workspace`, its library
  `WorkspaceToolLibrary`, named `tool_workspace` as the namespace of its texts is;
  `caelumora-mcp` is `caelumora-tool-mcp`, its library `McpToolLibrary`, named `tool_mcp`.
  CRATES.md says how a crate is named and records the old names; the component kinds keep theirs.
