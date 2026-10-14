# 344 — The `WorkspaceManager` owns workspaces and also what is done in them

**Status:** done
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
- **Making a workspace in a session's scope is not asked about.** For `WorkspaceManager::create`
  the store asks `CreateWorkspace`, owned by the caller, but nothing asks whether they may read the
  session whose scope it lies in. What lies there keeps the session from being deleted by whoever
  may not delete it (`ScopeNotEmpty`), so a stranger who knows a session's id could keep its owner
  from deleting it. Only the REST route refuses a session's scope today (`ScopeKind::of_user`); a
  plugin calling the manager is not stopped. Issue [227](227-agent-management.md) closed the same
  gap for agents: `AgentManager::create` asks `GetSession` or `GetChannel` first. For workspaces
  the template is in the way: it makes the session's workspace before the session is stored
  (`template::resolve_workspace`), as it made its agents before 227; it would have to attach the
  workspace once the session is made, as it now takes in its agents.

## Done when

- It is decided whether the use of a workspace is split from its management; if so, the
  `WorkspaceManager` owns the lifecycle and the attributes of workspaces alone, and the documents
  name the owner of the rest.

## History

- **2026-10-13** — Opened while working on [227](227-agent-management.md), from a look at what the
  managers of the runtime own: the `GroupManager`, `SkillManager`, `ArtifactManager` and
  `AgentManager` own the lifecycle of their kind and, without a controller, its attributes; the
  `WorkspaceManager` owns more.
- **2026-10-13** — Noted while working on [227](227-agent-management.md): `WorkspaceManager::create`
  does not ask whether a workspace may be put into a session's scope, which 227 now asks for
  agents.
- **2026-10-14** — Decided with the maintainer: a workspace gets a controller, as sessions,
  channels, agents and users have one - the entity a plain struct in the model, its attributes its
  controller's, its lifecycle its manager's, and something running that holds it live. The
  `WorkspaceController` changes what a workspace is - `rename`, `describe`, `reclassify`,
  `set_access` and `share` move to it - and the `SessionLoop` of every session it is attached to
  holds it, which is what the rule of ARCHITECTURE.md asks of a kind with a controller. As a
  workspace of its owner's or the global scope may be attached to several sessions and is used
  directly through REST, there is one per workspace and instance, kept by the `ControllerManager` as
  those of agents and users are; a `SessionController` is a session's own only because a session
  runs once.
  Terminals are reached through the live workspace, not the terminal: a `Terminal` is a handle its
  storage makes at each call and knows nothing of who may use it. The controller holds the
  workspace's provider - which `reclassify` needs for its `destination` anyway - and hands out its
  terminal to whoever may use it directly (`terminal`, `has_shell`). Whether a workspace may be used
  in a channel (`terminal_in`, `shell_in`, `usable_in`) is asked on the channel's side, which holds
  the session, the channel and the workspace live, and which makes the channel the scope of a shell.
  `kill_command` only reaches a shell to end a command on it, which its caller does itself. The
  `WorkspaceManager` keeps making, deleting, restoring and purging workspaces with their places.
  Option A of the considerations - a second service for the use - and a type of its own for the
  storages are not followed: the map of storages is shared by the manager, which makes and removes
  places, and the `ControllerManager`, which gives a controller its provider.
  In steps, each without a change of behaviour: (1) the controller, for the attributes; (2) reaching
  terminals and shells through it and the channel, `kill_command` out, the `SessionLoop` holding the
  controller - not before, since nothing would read it; (3) perhaps checking the commands in the
  `SessionLoop`, which runs the channels they are started in. The gap of the session's scope noted
  on 2026-10-13 stays open with this issue.
- **2026-10-14** — First step: the `WorkspaceController`
  (`caelumora-runtime/src/controller/workspace.rs`) changes what a workspace is - `update_name`,
  `update_description`, `update_classification`, `update_access`, `share` - and the
  `WorkspaceManager` no longer does. Each change still reaches the workspace as stored, in the
  caller's name, before anything else is asked, and is applied to it as stored, not to the live
  copy: a change made elsewhere, an access list above all, may not have reached that yet. The
  `ControllerManager` keeps one per workspace (`workspace_controller`) and reads one again as stored
  at every announced change, since that of an access list does not carry it. It shares the map of
  storages with the `WorkspaceManager`; the check of a classification against the storage and the
  policy is one function for both (`workspace::require_classification`). The REST routes and the
  sessions that share a workspace change it through its controller; what is asked is unchanged.
  Nothing holds a controller beyond a call yet - the `SessionLoop` does with the second step.
- **2026-10-14** — Decided with the maintainer while working on the second step: what ends commands
  is a component of its own, the `CommandReaper` (`caelumora-runtime/src/workspace/reaper.rs`), held
  by the `RuntimeManager` - neither the `SessionLoop`, which would have to follow users and the
  workspace and end the commands of a workspace taken away, nor the `SessionManager` or the
  `WorkspaceManager`, which would only hand it what it needs. It ends the commands nobody may run
  any longer, asking of the session and the channel as stored, as before, and those of a stopped
  session's channels. Whether a workspace may be used in a channel is asked on documents, so that a
  view of a session that does not run asks it too: by the `WorkspaceController` (`usable_in`,
  `usable_in_channel`, `terminal_in`, `shell_in`), given the session and the channel.
  Second step: the `WorkspaceController` hands out the terminal of a workspace used directly
  (`terminal`) and in a channel, and says whether it has a shell; the `SessionLoop` holds the
  controller of its workspace and changes it with `set_workspace`. A channel reaches the workspace
  of its session with the session and the channel as they are now
  (`ChannelLoop::workspace_terminal`, `workspace_shell`), and a turn or an external agent's work
  through its channel (`AgentWork::workspace_terminal`, `workspace_shell`): the terminal tool, the
  artifact tool and the workspace prompt part do, the last two no longer using the
  `WorkspaceManager`. `kill_command` is gone; the REST route reaches the channel as other channel
  routes do (`open_channel`) and ends the command on its shell. The `WorkspaceManager` keeps making,
  deleting, restoring and purging workspaces with their places, finding one by name, the storages it
  lists and the largest file. What a caller is asked is unchanged, but for the REST route ending a
  command, which now asks whether the user may read the channel first, as every channel route does,
  and answers a channel that is not the session's as `channel_not_found`.
- **2026-10-14** — Decided with the maintainer: a turn, not the work it is part of, hands out the
  workspace's terminal and shell to act on (`Turn::workspace_terminal`, `Turn::workspace_shell`),
  which the terminal and the artifact tool use; `AgentWork` is the work to read, and gives the
  terminal for reading alone (`AgentWork::workspace_for_reading`), which the workspace prompt part
  uses. Reaching them through the views was not followed - a view reads and decides nothing -, nor a
  service looking the channel up by its id. A terminal reached for reading can still write: what is
  asked is which permission, not what the handle can do.
- **2026-10-14** — Done, with the gap of the session's scope closed here at the maintainer's request
  rather than in an issue of its own: `WorkspaceManager::create` asks, for a session's scope,
  whether its maker may make it (`CreateWorkspace`) and then may read the session (`GetSession`), as
  `AgentManager::create` does since [227](227-agent-management.md). So that a template is no
  exception to it, `SessionManager::create_from_template` finds or makes the template's workspace,
  or one chosen to be made for the session alone, once the session is made, and attaches it
  (`SessionLoop::attach_workspace`, which `set_workspace` uses too) before it takes in the agents;
  if that fails, the session is taken back, and a workspace made for it outside its scope with it.
  One of the user's own chosen as the session is made is still attached as it is made. The REST
  server still makes no workspace in a session's scope (`invalid_scope`).
