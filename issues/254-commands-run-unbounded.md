# 254 — A command handed back as running runs until the model ends it

**Status:** done
**Area:** Tools and approvals
**Summary:** A command an agent's call hands back as running runs until the model ends it - past its turn, its session and the channel's access to the workspace.

## The problem

`terminal_tool` (issue [251](251-open-terminal.md)) follows a command for at most `max_wait_secs`
and hands one still running back with its id; the model follows it with `command_output` and ends
it with `kill_command`. Nothing else ends it. A command lives with the storage's shell, not with a
call or a turn, so that it can outlive the call that started it - and so it also outlives

*   the turn, the agent's task and the session: a session stopped, an agent removed from its
    channel, a turn given up leave the command running;
*   the access it was started under: a channel that stops using the workspace
    (`Channel::uses_workspace`), a workspace taken from the session, reclassified or deleted - the
    command goes on reading and writing its files, although no member of the channel could start
    it there any more;
*   Caelumora itself, for the `local` shell: its process groups are not killed when the server
    ends, and after a restart nothing knows them.

Only a call dropped while it waits kills its command (251, part C4). A model that forgets a command,
or a turn that ends before the model ends it, leaves it to run for ever.

## Why it matters

A command is an effect of a call that was approved for one turn; one that runs on is an effect
nobody approved any longer, and one that runs on after its channel lost the workspace crosses the
boundary the switch and the flow rule draw. On a shared host it also holds processes, memory and
the workspace's files - a `remove_dir_all` of a purge races a command still writing.

## Considerations

- **A bound per command.** A `max_run_secs` - of the tool, or of the storage - after which the shell
  kills it. Simple and the same for every provider; Open Terminal has no such bound of its own,
  so the shell would keep the timer. It bounds the damage, not the crossing of the boundary.
- **Ending a scope's commands when its access ends.** The shell kills what was started under a
  scope (a channel) when the channel stops using the workspace, the workspace leaves the session,
  the session stops or the workspace is purged: a `Shell::kill_all(scope)` or a provider's
  `stop(workspace)`, called by whoever changes that access. It needs the owner of each of those
  changes to know of the shell, or an event the shells listen to.
- **Ending everything at shutdown.** The `local` provider kills its process groups when it is
  dropped, and a restart finds no orphans; Open Terminal's containers are 251's step 7.
- The three are not exclusive; the second is the one that keeps the boundary.

## Decisions

Taken with the maintainer on 2026-10-10. It comes after
[296](296-workspace-operations-on-the-view.md), on its shell bound to its scope.

- **A command is its channel's, not its starter's** - as [294](294-user-ends-a-command.md) decided
  for ending one. It runs on as long as some current member of the channel could have started it
  there: the workspace usable in the channel for them (`WorkspaceManager::usable_in`) and its files
  writable. An agent removed from the channel leaves its command to the members who remain, who see
  it and may end it. So the rule is kept once, in `usable_in`, and all a shell must know of a
  command is its workspace, its scope and its id.
- **The shell keeps the register, not the runtime.** Not a register in the `WorkspaceManager`
  beside the shells' own: what starts a command enters it, so that nothing starts one past it, and
  296's shell bound to its scope can be handed out as decided there. `LocalCommands` becomes a
  generic type of the `terminal` module - the commands of a storage's workspaces by workspace, scope
  and id, with when each was started - in which the `local` shell keeps its processes and the Open
  Terminal shell its ids. A provider lists them (`TerminalProvider::commands()`) and ends one
  without asking anything; a list per provider, not per workspace's shell, so that looking does not
  start a stopped container.
- **Open Terminal's own list does not do it.** Checked against 0.14.0: `GET /execute` lists only
  the processes whose `X-User-Id` and `X-Session-Id` are those of the request - the scopes must be
  known to list their commands -, and a process has neither its `cwd` nor a start time, so in an
  `open_terminal_shared` instance a command cannot be told to be in which workspace. The table lives
  in the instance's memory.
- **The runtime ends what may no longer run.** A task of the `WorkspaceManager`, woken by the events
  that can change the answer (`SessionUpdated`, `ChannelUpdated`, the `Workspace…` events, a member
  leaving) and besides at an interval, asks the rule above of every command the providers list - the
  channel found by the scope, which `get_channel` needs no session for - and ends those nobody may
  run there any longer. That a scope is a channel stays the runtime's, at the one place 296 gives
  it.
- **A bound per storage, `max_run_secs`**, with a default: the host's resources are the storage's;
  the tool's `max_wait_secs` is how long a call waits. The `local` shell kills a command's process
  group when its time is over. An Open Terminal shell starts not the command but
  `exec timeout -s KILL <max_run_secs> bash -c "$CAELUMORA_COMMAND"`, the command in the
  environment, so that the instance ends it by itself - also without Caelumora, and after a
  restart. GNU `timeout` (in the image: coreutils 9.7) signals its whole process group; a command
  that leaves the group (`setsid`) escapes the bound - it holds against forgetting, not against
  intent.
- **What a restart leaves is named, not closed.** The register lives in memory.
  `open_terminal_docker` stops at start what an earlier run left running. A command of an earlier
  run in an `open_terminal_shared` instance is found by nothing and ends only at `max_run_secs`; one
  of the `local` shell is not bounded at all - `local` is for development and tests. Both are said
  in [STORAGE.md](../docs/STORAGE.md).
- **Not decided:** whether stopping a session or a channel ends its commands - not a matter of
  access, a stopped session can be started again, but nothing of it runs any more; whether the
  server gets an orderly shutdown in which the `local` provider kills its process groups (dropped,
  `kill_on_drop` kills only `bash`, not its group); whether `LocalProvider::remove` ends the
  workspace's commands before it removes their directory.

## Done when

- No command runs on after the channel it was started from may no longer use the workspace, and
  none runs on unbounded.

## History

- **2026-10-08** — Opened with step 5 of 251's plan, where the local shell and `terminal_tool` were
  built: a command handed back as running is kept by the shell and ended only by the model.
- **2026-10-08** — In part: an `open_terminal_docker` storage (step 7 of 251's plan) stops a
  workspace's container after `idle_stop_secs` unused, and with it every command in it - a command
  handed back as running and never followed again ends then. Nothing changes for the `local` shell
  and `open_terminal_shared`.

- **2026-10-10** — A user who wants to end a command running in the background is a concern of its
  own, user control rather than a boundary of access: issue [294](294-user-ends-a-command.md).
  Interrupting a running call (issue [276](276-interrupt-one-tool-call.md)) reaches a command only
  while `run_command` waits for it.
- **2026-10-10** — Looked at with the maintainer: the decisions above recorded, after Open
  Terminal's `GET /execute` was checked against 0.14.0. They replace a register of commands in the
  `WorkspaceManager`, fed by a `run_command` moved there, which was weighed first.
- **2026-10-10** — Implemented as decided, all but the points not decided. `terminal::commands`
  holds the generic `Commands<C>`, by workspace, scope and id, with `end` for a provider ending one
  when told; the `local` shell keeps its processes in it, the Open Terminal shell what it started
  (`Started`). `TerminalProvider` lists the commands that may still run (`commands`) and ends one
  without asking (`end_command`); `open_terminal_docker` does not start a stopped container for it
  and forgets a workspace's commands when its container stops or is removed. `max_run_secs` (3600)
  is a field of every storage; the `local` shell kills a command's process group once it ran so
  long, an Open Terminal shell starts it under `timeout`, which the real instance was checked to do
  (`a_command_is_killed_by_its_container_once_it_ran_as_long_as_it_may`, Docker). The
  `WorkspaceManager` starts, where a storage has a shell, a task that ends the commands nobody may
  run any longer, woken by the events that can change the answer and every minute; whether a member
  could start a command there is `terminal_in`'s question for writing, now kept once in
  `usable_in_channel`. Besides the decisions: the runtime asks those questions in each member's
  name with the system as initiator, so that the record tells them apart; the security suite's
  `asked` leaves them out of an action's questions, and its own test records them (`recorded`,
  `a_command_runs_on_only_while_some_member_could_start_it_in_its_channel`). A command an Open
  Terminal instance ended by a signal after it ran as long as it may is told as `killed`, as the
  `local` shell tells it. A storage with a shell needs a running Tokio runtime, as a Docker storage
  did already. No stored data changes.
- **2026-10-10** — Done. The points not decided, decided with the maintainer: stopping a session
  ends the commands of its channels - `SessionManager::stop_running`, now async, has the
  `WorkspaceManager` end them (`end_commands_in`), directly rather than through `SessionStopped`,
  which the task may fall behind on and its round every minute could not make up for: whether a
  session runs is the `SessionManager`'s. A channel is stopped only when it is deleted, which the
  task covers already. Removing a place ends the commands running in it first
  (`TerminalProvider::end_commands_of`, for `local` and `open_terminal_shared`; a Docker storage's
  container goes with them). An orderly shutdown, which would end the `local` shell's commands when
  Caelumora ends, is worth having for more than commands: issue
  [297](297-orderly-shutdown.md).
