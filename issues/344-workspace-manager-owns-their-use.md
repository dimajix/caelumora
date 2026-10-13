# 344 — The `WorkspaceManager` owns workspaces and also what is done in them

**Status:** open
**Area:** Code structure

## The problem

The `WorkspaceManager` (`caelumora-runtime/src/workspace.rs`) makes, deletes, restores and purges
workspaces, and - as a workspace has no controller - changes what they are: `rename`, `describe`,
`reclassify`, `set_access`, `share`. It also:

- **reaches a workspace's terminal for a caller**, directly or from a channel (`reached`,
  `terminal`, `terminal_in`, `usable_in`, `has_shell`), asking the session, the channel, the
  workspace's access list and the confidentiality policy;
- **hands out the shell of a channel and ends its commands** (`shell_in`, `kill_command`);
- **ends the commands nobody may run any longer**, in a task of its own, and those of a stopped
  session's channels (`end_commands_in`, issue [254](254-commands-run-unbounded.md));
- **holds the storages** (`storages`, `max_file_bytes`).

For that it holds, beside its own store, the stores of sessions, channels and groups, the
confidentiality policy and the `ControllerManager`. Its description needs an "and also": it owns
workspaces, and also what is done in them. Its callers for the second part are the terminal tool,
the artifact tool, the workspace prompt part, the channel's and the workspace's views, the
`SessionManager`, the `SessionLoop` and the REST routes of files and commands.

## Why it matters

- **One owner per concern** (AGENTS.md): who looks for who may use a workspace in a channel, or
  what ends a command, finds it among making and renaming workspaces.
- **The rule of managers and controllers** (ARCHITECTURE.md, set with issue
  [227](227-agent-management.md)): a manager owns the lifecycle of its kind and, where the kind has
  no controller, what it is. The first half of the `WorkspaceManager` is that; the second is
  neither.
- **Most of what it holds serves the second half.** Its lifecycle and attributes need its store,
  the event bus, the security manager and the groups an access list names.

## Considerations

- **A. Split the use of a workspace from its management.** A type of its own - name to be decided -
  reaches terminals and shells for a caller and ends commands, holding the storages, the stores of
  sessions and channels, the confidentiality policy and the controllers; the `WorkspaceManager`
  keeps lifecycle and attributes. Costs: a second service, which the runtime builds and hands to the
  callers above; `reached` is needed by both - reaching a workspace is the views' and the store's
  question, so it may belong to neither.
- **B. Leave it**, and say in the manager's documentation that it owns the use of workspaces too.
  Cheapest; the "and also" stays.
- **Where the storages belong.** The storages and ending commands may belong with the terminal
  (`caelumora-runtime/src/terminal/`) rather than with either type.
- Issue [333](333-workspaces-woven-into-sessions.md) decided that workspaces stay in the core; this
  is not about moving them out.

## Done when

- It is decided whether the use of a workspace is split from its management; if so, the
  `WorkspaceManager` owns the lifecycle and the attributes of workspaces alone, and the documents
  name the owner of the rest.

## History

- **2026-10-13** — Opened while working on [227](227-agent-management.md), from a look at what the
  managers of the runtime own: the `GroupManager`, `SkillManager`, `ArtifactManager` and
  `AgentManager` own the lifecycle of their kind and, without a controller, its attributes; the
  `WorkspaceManager` owns more.
