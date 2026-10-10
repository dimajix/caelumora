# 296 — The view of a workspace, which is for reading, carries out what is done with it

**Status:** done
**Area:** Code structure

## The problem

A frontend reads through the runtime's views; what changes something is carried out by its owner,
each operation checking itself ([SECURITY.md](../docs/SECURITY.md#views): "The manager carries out
what changes an artifact"). `WorkspaceView` does not keep to that:

*   `terminal(need)` hands out the workspace's terminal, for changing its files too (issues
    [253](253-workspace.md), [23](23-acls-in-security-attributes.md)). Everything done with a
    workspace's files goes through it: the file and shell tools of `terminal_tool`,
    `workspace_prompt_provider`, and the REST handlers of `/workspaces/{id}/files`, uploading and
    deleting among them.
*   `publish_artifact` (issue [271](271-publishing-artifacts.md)) reaches the terminal and has
    `ArtifactManager::publish` publish from it.
*   `kill_command` (issue [294](294-user-ends-a-command.md)) reaches the terminal for writing and
    ends a command of the channel.

Each of them asks again when it acts and so relies on nothing the view holds; but they sit on the
view because only the view knows how the workspace was reached (`Way::Direct` or `Way::InChannel`),
and each new thing done with a workspace in a channel is added there.

Two things next to it have the same cause:

*   **The scope of a command is made by every caller.** `Shell::start`, `output` and `kill` take the
    scope with every call, and the rule that a command is its channel's - the scope is the channel's
    id - is kept twice: in `WorkspaceView::kill_command`, and in `terminal_tool`'s `shell::execute`
    from `turn.channel_id()`. `KillOnDrop` carries the shell and the scope apart.
*   **`ArtifactManager::publish` takes a terminal**, the workspace and a path, to read one file -
    and so knows terminals and workspaces, and has to be given one reached the right way. What it
    keeps is content; where the content comes from is its caller's. Reading the file before the
    artifact's own permission is asked gives nothing away: the terminal was reached only by who may
    read the workspace, and a refused publish keeps nothing.

## Why it matters

A view that acts is a second place where operations live, beside the managers, chosen by whoever
adds the next one; a reader cannot tell any more from the kind of an object whether it changes
something. The scope made by each caller is a rule kept twice, which a third caller can get wrong
without anything noticing. And issue [254](254-commands-run-unbounded.md) is about to add
`kill_all(scope)` and a list of a scope's commands - a third and fourth use of the same scope.

## Considerations

- **Allow it and say so.** The documents could say that a view may offer an operation that asks
  everything again. Nothing to change, but the line between reading through the views and acting
  through the owner blurs, and it invites the next operation onto a view.
- **Act through the owner.** The caller reads through the views - the session and the channel,
  asking `GetSession` and `GetChannel` - and acts through the `WorkspaceManager`, which owns the
  workspaces and their terminals and already holds what decides whether a workspace is usable in a
  channel (`usable_in`) and reaches one directly (`reached`). The same questions are asked, split
  into reading and acting; the security suite records them.
- **The shell bound to its scope.** Instead of a scope with every call, `Terminal::shell(scope)`
  gives a shell that sees the commands of that scope alone; `start`, `output` and `kill` lose the
  parameter. The scope is made in one place, by what hands out the shell in a channel. A wrapper in
  the runtime (`ChannelShell { shell, scope }`) or a tuple from the view would make it in one place
  as well, but the first is a type around a type and the second still lets a caller pass another
  scope. The storages' shells change (`LocalShell` keeps the scope as a field instead of in its key,
  the Open Terminal shell for its header), and their tests. For a storage the scope stays any text;
  that it is a channel is the runtime's.
- **`KillOnDrop` asks nothing again.** A dropped call kills its command through the shell it holds,
  as now. Killing through an operation that asks everything again would fail exactly where a turn
  is cancelled because its agent was removed from the channel, and leave the command running. The
  shell is reached once for a call, as a terminal is now.
- **`publish` takes the content.** `ArtifactManager::publish` is given the size and the stream of
  what to keep and the classification it has, instead of a terminal, a workspace and a path.
  `TooLarge`, the digest and `Changed` work on them as now.
- **Not part of this:** a terminal reached for reading can still write - the grant decides what is
  asked, not what the object can do. That is a question of its own.

## Decisions

Taken with the maintainer on 2026-10-10, when the issue was opened.

- **Act through the owner.** `WorkspaceView` keeps what is read - its name, classification,
  storage, description, `has_shell`, `max_file_bytes`, `may_change`, `may_write`, `snapshot` - and
  loses `terminal`, `publish_artifact` and `kill_command`; what they did, the `WorkspaceManager`
  does.
- **The shell bound to its scope**, by `Terminal::shell(scope)`, the scope made where the shell of
  a workspace in a channel is handed out.
- **`publish` takes the content** and the classification.
- A restructuring without a change of behaviour, a step of its own; it comes before 254.
- **A tool reaches the managers through its `ComponentContext`**, decided on the same day once
  it turned out that a tool's only way to the runtime's services is the views of its turn - the
  reason the operations were put on the view. The context holds the services of the runtime by
  their type, not as fields of its own: `context.service::<WorkspaceManager>()`. A kind declares
  what it uses (`ComponentKind::uses::<T>()`, beside `provides`), and its factory is given those
  alone. The `SecurityManager` and the `SecretManager`, fields of the context so far, become
  services as well. Keyed by type rather than by name: the caller needs the type to use the
  service anyway, and a name would be a second key that has to match it.
- **Issue [128](128-actor-runtime-in-components.md) is loosened by it**: a component given the
  `WorkspaceManager` could act with it as its agent outside of a turn. It could do so with the
  views of a turn it keeps already; what a kind may use is now at least written down where it is
  registered.

## Done when

- No method of a view changes anything or hands out what does; the documents say so.
- The scope of a command is made in one place, and `Shell` takes none.
- `ArtifactManager::publish` knows no terminal.
- The security suite asks the same questions as before.

## History

- 2026-10-10: Opened while looking at [294](294-user-ends-a-command.md) with the maintainer:
  `kill_command` was the third operation added to `WorkspaceView`, and the scope of a command was
  made by two callers.
- 2026-10-10: The way a tool reaches the managers decided with the maintainer (above).
- 2026-10-10: Done, in four steps. (1) Components are given the runtime's services by type
  (`ComponentServices`, `ComponentKind::uses`, `ComponentContext::service`); the `SecurityManager`
  and the `SecretManager` are services, the `WorkspaceManager` and the `ArtifactManager` too.
  (2) A shell is bound to its scope (`Terminal::shell(scope)`); the Open Terminal shell is a type of
  its own. (3) `ArtifactManager::publish` takes the content and its classification - `pub`, since
  the security suite publishes as `artifact_tool` does, the caller vouching for the
  classification. (4) What is done with a workspace is the `WorkspaceManager`'s: `terminal`
  directly, `terminal_in`, `shell_in` and `kill_command` in a channel, by ids. Different from what
  was laid out: the manager does not take the session and the channel from the caller - a frontend
  has no documents -, but reads them itself and asks as the views ask; the question how a channel
  is read moved from `SessionView::readable` to `AccessQuestions::channel_reading`, which both ask.
  The suite asks the same questions, once each: the kill of 294 records the same sequence. The
  views are read alone: `WorkspaceView` lost `terminal`, `publish_artifact` and `kill_command`, and
  how it was reached; `tool::channel_workspace`, `ArtifactError::NotInChannel`, `Workspace` and
  `Terminal` are gone, and `WorkspaceError::NotInChannel` became `NotUsableInChannel` (REST
  `404 workspace_not_found`). All three shell tools reach their shell through `shell_in`;
  `WorkspaceManager::kill_command` is REST's.
