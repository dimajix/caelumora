# 254 — A command handed back as running runs until the model ends it

**Status:** open
**Area:** Tools and approvals

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

## Done when

- No command runs on after the channel it was started from may no longer use the workspace, and
  none runs on unbounded.

## History

- **2026-10-08** — Opened with step 5 of 251's plan, where the local shell and `terminal_tool` were
  built: a command handed back as running is kept by the shell and ended only by the model.
