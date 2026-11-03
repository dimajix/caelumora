# 533 — The tools on attachments are part of the agents crate

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** The component kind `attachments`, whose tools reach the attachments an agent heard and copy them into the workspace, is part of the agents crate, while the tools on a subject of their own - the workspace, images, MCP - are libraries of their own.

## The problem

`caelumora-agents` holds the agent's cognitive cycle and the machinery of its tools - the facet
`ToolProvider`, the `ToolExecutor`, what tools share (`page_of`) - and beside them the built-in tool
components. Since issue [428](428-workspace-tools-in-a-crate-of-their-own.md) the line between them
has been: the tools that concern the agent and its turn stay - `ask_user`, `turn_end`, `sleep`,
`echo`, `calculator` -, the tools on a subject of their own are libraries of their own -
`caelumora-tool-workspace`, `caelumora-tool-image`, `caelumora-tool-mcp`, and the tools of the
plugins `caelumora-skills` and `caelumora-artifacts`.

The component kind `attachments` (issue [282](282-message-attachments.md)) is on the other side of
that line and still in the agents crate:

- `caelumora-agents/src/tool/attachments.rs`, about 590 lines with its test: `view_attachment`,
  `read_attachment`, `copy_attachment` and `attach`, which reach the attachments of the agent's
  channel through its view, and write one into the session's workspace;
- its texts in `caelumora-agents/src/prompts.yaml`, under `tools.attachments` of the namespace
  `agents`;
- the test M5 of `caelumora-agents/tests/security_suite.rs`, what reaching an attachment asks.

It is the only built-in kind that declares a workspace (`reads::<dyn WorkspaceReader>()`,
`writes::<dyn WorkspaceWriter>()`), the subject `caelumora-tool-workspace` was cut out for.

Raised by the maintainer on 2026-11-03.

## Why it matters

- **One owner per concern.** The agents crate is what an agent does in the runtime; "and also its
  tools on the channel's attachments and on copying them into a workspace" is the "and also" the
  guidelines warn of.
- **A pattern followed through.** A tool on a subject is a library built on the agents' public API;
  the attachments' would be the fourth `caelumora-tool-…`, not an exception beside them.

## Considerations

- **What stays in the agents.** The numbering of attachments is the transcript's, not the tool's
  (see [AGENT-TURN.md](../docs/AGENT-TURN.md#attachments)): the note with its `ref`, the
  `AttachmentIndex` in the agent's state, the numbering of what a tool gave its model
  (`ToolOutput::shown`, which the `ToolExecutor` handles for every tool). The note names no tool, so
  the core keeps naming none. Also staying:
  - `Turn::copy_attachment` and `Turn::put_aside`, the turn's way to copy an attachment for its
    answer and to give it along with the next one. `put_aside` then has its one caller outside the
    agents crate - as `Turn::workspace_shell` had after 428 -, and stays as a concept of the turn.
  - `page_of`, which `read_file` of `caelumora-tool-workspace` shares.
  - `impl From<AttachmentError> for ToolError`: the orphan rule keeps it beside
    `From<TerminalError>` and `From<ViewError>`, as 428 found.
- **What the crate needs of the agents** is public already, as `caelumora-tool-workspace` uses it:
  `ToolProvider`, `ToolError`, `ToolOutput`, `ToolResult`, `arguments`, `described`, `tool_text`,
  `page_of`, `workspace_reader`, `workspace_writer`; `Situation` with `agent_channel_state`,
  `channel` and `profile`; `Turn` with `copy_attachment` and `put_aside`; of the prompts `texts_of`
  and `writing_for_the_model`. No visibility has to widen.
- **Names.** After the scheme of [CRATES.md](../docs/CRATES.md#how-a-crate-is-named) and issue
  [531](531-workspace-tools-named-apart-from-the-tools.md): the crate `caelumora-tool-attachment`,
  singular as the kind of crate; its library `AttachmentToolLibrary`, named `tool_attachment`, as
  the namespace of its texts is. The component kind keeps its name `attachments` - after issue
  [532](532-component-kinds-named-after-their-facet.md) a kind is named after what it gives the
  agent -, so that no personality and no seed changes (`examples/seed.yaml` configures it). Its
  module follows the other crates of tools: `component.rs` with the kind and its tool names,
  `library.rs`, `prompts.yaml`, the namespace and the texts in `lib.rs`.
- **Texts.** `tools.attachments` moves from the namespace `agents` to `tool_attachment`. A variant
  of the texts under `prompts` that overrides one of them in its `agents.yaml` has to move it to a
  `tool_attachment.yaml`; the check of the variants refuses the old entry as a text the defaults do
  not have, so that a forgotten one fails at start, not silently.
- **Tests.** The unit test moves with the kind; it uses `caelumora_agents::testing` (`turn`,
  `agent_runtime`, `prompts_with`), `Turn::of` and the kernel's `testing`, which the crate takes as
  dev-dependencies with `test-support`, as `caelumora-tool-workspace` does. The test M5 of the
  agents' security suite moves to the crate's own tests, in the suite's world, as the workspace
  tools' did (`caelumora-tool-workspace/tests/workspace_tools.rs`).
- **Programs and scripts.** `caelumora-app` and `caelumora-tui` list the library;
  `scripts/crates.sh` lists it among the libraries, so that `scripts/library-deps.sh` checks it and
  `scripts/crate-graph.sh` draws it.
- **Without the library.** A program that does not link it still numbers attachments and gives
  images along to a model that sees; the agent only cannot reach what it heard by a tool, as it
  cannot reach a workspace without `caelumora-tool-workspace`. Both programs link it, so nothing
  changes for them.
- **Not part of this issue.** `channel_history` (`read_history`) in the agents' `components`
  reaches other channels' messages - a subject of the kernel too, but given as the agent's
  context. Whether it follows is a question of its own, if the maintainer wants it raised.
- **Related.** Issue [465](465-attachments-in-the-context.md) (open) decides what of an attachment
  goes into the context and what the model fetches through these tools. Should the note in the
  transcript then name the tool to fetch with, the core would name a tool of a library - as the
  workspace's prompt part would have in 428, which is why it moved with its tool. That is for 465
  to keep in mind; it does not block this one.

## Done when

- `caelumora-tool-attachment` holds the component kind `attachments` with its texts and its tests;
  `caelumora-agents` registers it no more, its `prompts.yaml` holds no text of it, and its
  security suite no test of it.
- `caelumora-app` and `caelumora-tui` list the library; a component configured as `attachments`
  works as before.
- `docs/ARCHITECTURE.md`, `docs/COMPONENTS.md`, `docs/TOOLS.md` and `docs/CONFIGURATION.md` (the
  namespaces of the texts) say where the kind lives; `docs/CRATES.md` draws the crate.

## History

- **2026-11-03** — Opened at the maintainer's request.
- **2026-11-03** — Done. `caelumora-tool-attachment` holds the kind `attachments` in its
  `component.rs`, its texts in its `prompts.yaml` under the namespace `tool_attachment`, and
  `AttachmentToolLibrary`, which `caelumora-app` and `caelumora-tui` list; `caelumora-agents`
  registers the kind no more. Nothing of the agents had to become public. One thing differs from
  the considerations: the test M5 is no test of the crate's `tests/`, but a unit test beside the
  kind's own, in the suite's world (`caelumora_agents::testing::world`) and with the same
  session, as `caelumora-tool-workspace` tests what its reading asks
  (`a_read_of_a_file_asks_what_reading_asks_and_never_a_change`). Moved as it stood into `tests/`,
  it would have made the component, its constructor, its tool names and its texts public for the
  test alone, where the crate keeps them to itself, as `caelumora-tool-image` does. Rewritten
  through the whole kernel - a scripted model calling the tool, as the workspace tools' tests in
  `tests/` do - it would have needed nothing public but the kind's name, but it would assert other
  questions - those of the turn, asked by the one who woke the agent -, so that a refactoring would
  have changed a test of the suite, which is reviewed as a change of the permissions an action
  needs; and the agents' `testing` has nothing yet that has an agent hear an attachment first.
