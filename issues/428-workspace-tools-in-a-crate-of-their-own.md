# 428 — The tools of the workspace are part of the agents crate

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** The `terminal_tool` and the `workspace_prompt_provider`, which give an agent the session's workspace, are part of the agents crate, while the tools of other subjects - artifacts, skills, MCP - are plugins.

## The problem

`caelumora-agents` holds the agent's cognitive cycle and the machinery of its tools, and beside
them two component kinds that give an agent the session's workspace (issue
[251](251-open-terminal.md), part C):

- `terminal_tool` (`caelumora-agents/src/tool/terminal_tool/`, about 1350 lines with tests): the
  workspace's files - `list_files`, `read_file`, `grep_files`, `write_file`, `replace_in_file`,
  `move_file`, `delete_file`, `publish_file` - and its shell - `run_command`, `command_output`,
  `kill_command`;
- `workspace_prompt_provider` (`caelumora-agents/src/prompt/workspace_prompt_provider.rs`): which
  workspace the agent works on, whether commands run there, and the instruction files at its root,
  which it asks the agent to read "with `read_file`".

The other tools the agents crate brings - `ask_user_tool`, `stop_tool`, `sleep_tool`, `echo_tool`,
`calculator_tool` - concern the agent and its turn. The tools of a subject of their own live with
that subject: the artifact tool in `caelumora-artifacts` (issue
[328](328-artifacts-in-a-crate-of-their-own.md)), the skills' in `caelumora-skills`, `mcp_tool`
in `caelumora-mcp` (issue [325](325-mcp-in-a-crate-of-its-own.md)). The workspace's are the
exception.

## Why it matters

- **One owner per concern.** The agents crate is described as what an agent does in the runtime;
  "and also its tools on workspaces" is the "and also" the guidelines warn of.
- **Dependencies.** `regex` is used by `grep_files` alone.
- **A pattern followed through.** A tool on a subject is a plugin built on the agents' public API,
  as the artifact tool shows; the workspace's would be the second of its kind, not an exception.

Workspaces themselves stay in the core (issue [333](333-workspaces-woven-into-sessions.md),
rejected): this moves what an agent is given to work on them, not the workspaces.

## Considerations

- **The prompt part goes with the tool** (decided 2026-10-18). `workspace_prompt_provider` names
  `read_file`, a tool of `terminal_tool`, and says whether the agent can run commands; left in the
  agents crate, the core would name a tool of a plugin. The alternative - a prompt part that names
  no tool ("read it first") - was weighed and set aside: it tells the model less, and the two parts
  belong together by subject. Both are configured beside each other already (`examples/seed.yaml`).
- **What the crate needs of the agents** is public already, as the artifact tool uses it: `Turn`
  with `workspace_terminal` and `workspace_shell`, `Situation` with `workspace`,
  `usable_workspace`, `ApprovalMode`, `ToolProvider`, `ToolError`, `ToolOutput`, `ToolResult`,
  `arguments`, and of the prompts `PromptProvider`, `Prompt`, `PromptPosition`, `ContextError`,
  `titled`. Only `proposed_matcher`, with which the tool proposes a rule for a call it asks about,
  is `pub(crate)` and becomes public. `Turn::workspace_shell` then has its one caller outside the
  agents crate; it stays where it is, as the turn's way to the shell of its channel's workspace.
- **Tests.** The unit tests of both move with them; they use `session_with_workspace` and the
  runtime's `testing`, which the crate takes with the feature `test-support`. The tests of the
  terminal tool in `caelumora-agents/tests/security_suite.rs` move to the crate's own tests, in
  the suite's world (`caelumora_agents::testing`), as AGENTS.md asks of a plugin.
- **Names.** The crate `caelumora-workspace-tools`, since it holds a tool and a prompt part; its
  plugin `WorkspaceToolsPlugin`, named `workspace_tools`, registering both kinds in
  `Plugin::register`. The kinds keep their names, `terminal_tool` and
  `workspace_prompt_provider`, so that no configuration and no seed changes; the programs list the
  plugin.

## Done when

- `caelumora-workspace-tools` holds `terminal_tool` and `workspace_prompt_provider` with their
  tests; `caelumora-agents` registers neither and no longer depends on `regex`.
- `caelumora-app` and `caelumora-tui` list the plugin; components configured as `terminal_tool`
  and `workspace_prompt_provider` work as before.
- `docs/COMPONENTS.md`, `docs/TOOLS.md` and `docs/ARCHITECTURE.md` say where the two kinds live.

## History

- **2026-10-18** — Opened from the maintainer's question whether the terminal tool could move into
  a crate of its own, asked after issue [427](427-local-terminal-in-a-crate-of-its-own.md) was
  rejected - where moving it would not have helped. Decided with it: the workspace prompt part
  moves along.
