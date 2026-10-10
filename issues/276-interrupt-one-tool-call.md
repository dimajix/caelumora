# 276 — A tool call that hangs cannot be interrupted on its own, only with its turn

**Status:** open
**Area:** Tools and approvals

## The problem

Since issue [161](161-tools-cannot-be-interrupted.md), a running tool call can be interrupted - but
only by ending the turn it belongs to: the stop button of the channel (`POST …/cancel`) or of one
agent (`POST …/participants/{aid}/cancel`, issue 235) cancels the turn's token, the call's future is
dropped and answered as `Interrupted`, and the turn is given up. There is no way to break off a
single call and let the agent go on.

An example: an agent starts a shell command (`run_command`) that waits for something that never
comes - input, a lock, a network peer. The call follows it for up to `max_wait_secs` (60 unless
configured, at most 300). The user sees that it hangs, but can only throw away the whole turn - with
what the agent had worked out so far, and with the agent left without knowing what went wrong, so
that the next turn has to start over. An MCP call or any other tool that hangs is bounded by nothing
at all.

## Why it matters

A hanging call is a problem of the call, not of the turn. The agent could deal with it well - try
the command with `-y`, without a pager, with a timeout, or ask the user - if it were told that the
call was broken off and why. Ending the turn instead loses work and puts the user in the position of
having to explain in a new message what the agent would have seen in a tool result.

## Considerations

**Interrupt the call, keep the turn.** The mechanism of 161 is already in place:
`GuardedToolExecutor::execute` `select!`s the executor's future against a token and answers the
call as `Interrupted` when it fires. A token per running call - a child of the turn's token, so
that ending the turn still ends the call - kept by the agent's `RunControl` next to the turn's
token, and the call interrupted when it fires. The difference to a cancel is what follows: the
result goes to the model as any other result, saying that a user interrupted the call (and who,
and after how long), and the turn goes on with its next step.

- **Effect of the interruption.** Dropping the future is what a cancel does already; for
  `run_command` it kills the command (251, part C4: a call dropped while it waits kills its
  command). A command already handed back as running is no running call any more - ending it is
  `kill_command`'s, for the model, and the problem of commands that run on is issue
  [254](254-commands-run-unbounded.md).
- **Addressing.** By the agent and the call's id, the one the transcript and the web UI know: e.g.
  `POST …/participants/{aid}/tool-calls/{call_id}/cancel`, through the channel to the actor as
  `cancel_actor` does. A call not running (any more) is answered as such, not as an error that ends
  anything.
- **Permission.** The same as cancelling the agent's turn (`CancelChannel`): who may end the turn
  may end a part of it. A new action in someone's name, so with its test in the security suite.
- **Status.** `Interrupted` as now, or a status of its own that tells the model the turn goes on -
  to be decided; the text of the result says it either way.
- **Concurrent calls** (issue [226](226-concurrent-tool-calls.md)): a token per call fits calls
  that run side by side as well as one after the other.
- **Web UI.** A stop button on the call shown as "Executing" in `ToolCallView`, for those who may
  cancel the agent's turn. The terminal client could follow later.

**Alternatively or additionally: a bound per call.** A timeout of the tool or of the executor after
which a call is interrupted by itself. It helps when nobody watches, but needs a value that fits
every tool; it does not replace a user's judgement that a call hangs. Can follow separately.

## Decisions

Taken with the maintainer on 2026-10-10.

- **What it reaches, and what not.** Interrupting a call ends what the call does while it runs -
  for `run_command`, the command it waits for (`KillOnDrop`). A command handed back as running is
  no call any more: a `command_output` call that follows it is interrupted as a call, the wait
  ends and the command runs on. The model is told and can end it with `kill_command`; that the
  tool cannot tell why it was dropped is why it does not kill the command itself. A user who wants
  to end such a command is issue [294](294-user-ends-a-command.md); commands that run on unbounded
  are [254](254-commands-run-unbounded.md). Neither blocks this issue, nor this one them.
- **Addressing by turn and call id.** `ToolCall::id` is the model's: optional, and not unique
  across turns (Ollama answers `"1"`, `"2"`, … or none). A call is addressed by the agent, the turn
  (`TurnId`, which the web UI knows since issue 235) and the call's id, e.g.
  `POST …/participants/{aid}/turns/{turn_id}/tool-calls/{call_id}/cancel`, so that a request from
  an outdated view cannot reach a call of a later turn with the same id. The agent gives every call
  the model left without an id one of its own as it stamps the call (`stamp`/`label`), so that
  every call can be addressed. A call not running (any more), or of a turn not under way, is
  answered as such, not as an error that ends anything.
- **The reason reaches the executor with the interruption.** The result the model is given is made
  in the `ToolExecutor` alone, and a `CancellationToken` carries no reason. `execute` takes,
  instead of the turn's token, a future that ends with why the call is interrupted: the turn ended,
  or a user (who, after how long) interrupted it. That reason is a small enum of the tool module
  which also makes the text of the result, in place of `interrupted_result()`. The agent makes the
  future from its `RunControl`, which keeps a token per call - a child of the turn's - together
  with who interrupted it.
- **Every call of the step can be interrupted, also before it runs.** The web UI shows every call
  without a result as "Executing", also those of the step that have not begun. The tokens are made
  for all calls of the step as it begins; a call interrupted before it runs is not run and is
  answered as `NotExecuted` ("a user skipped it before it ran"). No event of a call beginning is
  needed for it, and it fits the concurrent calls of issue 226 as well.
- **The status stays `Interrupted`.** For the call the outcome is the same as when its turn ends -
  it ran, was broken off, and its effect is unknown; whether the turn goes on is the turn's
  matter. The text of the result says it: who interrupted it, after how long, and that the turn
  goes on.
- **Permission** as in the considerations: `CancelChannel`, with its test in the security suite.
- **A bound per call** is not part of this issue.

## Done when

- A user who may cancel an agent's turn can interrupt one of its running tool calls; the call is
  answered as interrupted, by whom, and the turn goes on with that result.
- The web UI offers it at the running call.
- The security suite covers the new action.

## History

- 2026-10-09: Opened at the maintainer's request: a hanging tool - e.g. a shell command waiting for
  input - can only be ended with its whole turn or agent, not on its own.
- 2026-10-09: With [280](280-tool-executor-trait.md), `GuardedToolExecutor` is called
  `ToolExecutor`; what is said of it above holds for it under its new name.
- 2026-10-10: Looked at with the maintainer together with [254](254-commands-run-unbounded.md):
  the decisions above recorded. The example of a shell command under *The problem* holds only while
  `run_command` waits for it; for a command handed back as running, issue
  [294](294-user-ends-a-command.md) was opened.
