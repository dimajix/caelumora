# 294 — A command running in the background can be ended only by the model, not by a user

**Status:** done
**Area:** Tools and approvals

## The problem

`run_command` of `terminal_tool` (issue [251](251-open-terminal.md)) follows a command for at most
`max_wait_secs` and hands one still running back with its id; from then on the command is no call
any more, but a command in the workspace's shell. Only the model ends it, with `kill_command`.

A user who sees such a command hang - waiting for input, a lock, a network peer - has no way to end
it:

*   Interrupting a running tool call, as issue [276](276-interrupt-one-tool-call.md) asks for,
    reaches a command only while `run_command` still waits for it: the dropped call kills it
    (`KillOnDrop`, 251 part C4). A `command_output` call that follows it is interrupted as a call -
    the wait ends, the command runs on.
*   Cancelling the agent's turn or the channel does not reach it at all: a command outlives its turn
    (issue [254](254-commands-run-unbounded.md)).
*   What remains is to tell the agent in a message to kill it - a turn of the model for something
    the user already decided, and only while the agent is there and willing.

## Why it matters

A command is started in a user's channel, in a workspace the channel uses, on a call that was
perhaps approved by that very user. That they can stop the call but not what it started is a gap in
the control they have over what an agent does in their name. Issue 254 bounds the commands nobody
ends any more; this one is about a person who wants to end one now.

## Considerations

- **Not 254.** 254 is about the runtime ending commands at the boundaries of access - a bound per
  command, the end of a scope's access, shutdown. Ending a command because a person decides to is
  user control, with its own action and permission. The two meet in the shell's `kill` and may
  share what lists a scope's commands.
- **Not 276.** 276 interrupts a call and lets the turn go on; a command handed back has no call.
- **Addressing.** A command is seen only under its scope, the channel it was started in, by its
  id (`Shell::kill(scope, id, offset)`). The `run_command` result that handed it back carries the
  id, so the web UI could offer "kill" there, as long as the command runs. A list of a scope's
  running commands - the `Shell` has none today - would show those whose result is long scrolled
  away, e.g. in the channel's workspace panel.
- **Permission.** Candidates: `CancelChannel` - who may end the agents' turns in the channel may
  end what they started there, the same argument as for 276; or `WriteWorkspace` - killing a
  process is a change to the workspace. The command is the channel's (its scope), which argues
  for the first; the workspace is the resource, which argues for the second. To be decided, and a
  new action in someone's name, so with its test in the security suite.
- **What the agent learns.** A command killed by a user is `Killed` the next time the model follows
  it (`command_output`). Whether the agent should be told otherwise - e.g. by a message in the
  channel - is open; ending it without telling would leave a model that does not follow it any more
  in the belief that it still runs.
- **Who it reaches.** Only commands of a workspace's shell; the terminal client and other shells
  could follow.

## Decisions

Taken with the maintainer on 2026-10-10.

- **Permission: `WriteWorkspace`, through the channel's workspace.** A command is reached only
  through the workspace as the channel offers it (`ChannelView::workspace()`: a member, the
  channel uses it, `GetWorkspace`, flow both ways); `CancelChannel` would have to go around that
  way, or be asked besides. Who may end a command is who could have started it there - the check
  `kill_command` makes for the agent already. A user who may only read the workspace cannot end
  one; they can still cancel the turn or ask the agent.
- **One place: `WorkspaceView::kill_command`.** For a workspace reached through a channel alone
  (`NotInChannel` otherwise, as `publish_artifact`): the terminal reached for writing, the command
  looked for under the channel's scope. The tool `kill_command` goes through it as well, so that
  the rule that a command is its channel's and the check for writing are kept once.
  `run_command` and `command_output` still make the scope themselves; moving them too is a step of
  its own, not part of this issue. REST:
  `POST /sessions/{id}/channels/{cid}/commands/{command_id}/kill`, answering the command's state;
  an unknown command is `404 unknown_command`, a workspace not usable here `404`, one only
  readable `403`.
- **The web UI offers it at the result.** A stop button at a result "Still running as …", for a
  user who may change the workspace's files (`can_write`), and gone once a later result in the
  channel shows the same command ended. A list of a scope's running commands (`Shell::commands`)
  for the workspace panel is not part of this issue; it belongs with
  [254](254-commands-run-unbounded.md), whose `kill_all(scope)` needs to know them as well.
- **The agent is told nothing besides.** It sees `killed` the next time it follows the command;
  one it no longer follows it does not need to know about. Who killed it (kept by both providers)
  or a notice in the channel would be a concept of its own.

## Done when

- A user who may do so can end a command of their channel that was handed back as running.
- The web UI offers it at the command.
- The security suite covers the new action.

## History

- 2026-10-10: Opened while looking at [276](276-interrupt-one-tool-call.md) with the maintainer:
  276 reaches a hanging command only while `run_command` waits for it, 254 only at the boundaries of
  access; a user who wants to end a command running in the background can do neither.
- 2026-10-10: Looked at with the maintainer: the decisions above recorded.
- 2026-10-10: Done. `WorkspaceView::kill_command(id, offset)` ends a command of the channel the
  workspace was reached through (`WorkspaceError::NotInChannel` for one reached directly), with its
  terminal reached for writing; the tool `kill_command` goes through it, and a workspace without a
  shell is answered as a command not known there. REST:
  `POST /sessions/{id}/channels/{cid}/commands/{command_id}/kill`, answering the command as a
  `run_command` result does. Different from the decision: a command not known in the channel is
  `404 command_not_found`, the code a `TerminalError::UnknownCommand` had already, not
  `unknown_command`, which names a user command that does not exist (`400`). The web UI shows a
  stop icon at a result "Still running as …" for `can_write`, until a later result or the user
  ended the command; `ChatView` now fetches the channel's workspace and hands it to
  `ChannelWorkspacePanel`. The security suite covers it
  (`a_command_is_ended_in_its_channel_by_who_may_change_the_workspace_there`).
