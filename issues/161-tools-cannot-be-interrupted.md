# 161 — A running tool cannot be interrupted, so a stop during it aborts the agent

**Status:** done
**Area:** Tools and approvals

## The problem

`ToolProvider::execute` and `ToolExecutor::execute` get the turn, not the turn's
`CancellationToken`, and nothing around them waits on it. A tool therefore runs to its end, whatever
happens meanwhile:

- **A cancel** waits for the tool. The turn ends only after it - correct, but a tool that runs for
  minutes (an MCP server that hangs) keeps the stop button red for minutes.
- **A stop** - the agent removed from the channel, or the channel deleted - also waits, but the
  channel does not: after `STOP_GRACE` (5 s, `channel.rs`) it aborts the actor's task. The turn is
  then neither given up nor taken over. What is stored is the state from before the tool ran
  (`ExecutingTools`, the call approved, no result). When the agent joins again, `prepare_resume`
  answers that call as "not executed" - although it ran, perhaps to its effect - and no dead branch
  records it, against what [145](145-ended-turns-leave-a-record.md) promises for every stop.

Found in the web UI while testing [152](152-three-signals-end-a-turn.md): removing an agent while
its `sleep` tool (30 s) ran logged "did not stop by itself … aborted" after 5 s.

## Why it matters

- The record of what a tool did depends on how long the tool takes.
- A hanging tool blocks cancelling for as long as it hangs.

## Considerations

- **Wait on the token around the call.** `GuardedToolExecutor::execute` `select!`s the executor's
  future against the turn's token; on the token the future is dropped and the call answered as
  interrupted, with the `started_at` it has - so the call is recorded as having run, and the turn
  can be given up or taken over. Dropping a future is not undoing its effect: an HTTP request
  already sent is sent ([20](20-transactional-effects.md)). The executor would need the token -
  through the turn, or as an argument.
- **Only for a stop, not for a cancel.** A cancel could keep waiting (the user asked to end the
  turn, not to break a tool off halfway); a stop cannot wait longer than the grace. Or both
  interrupt, and a tool that must not be cut off says so.
- **A longer grace** only moves the line.

## Done when

- A stop while a tool runs leaves the turn given up, with the call recorded as having started,
  however long the tool takes.
- Whether a cancel interrupts a tool is decided and documented.

## History

- **2026-09-30** — Found in the UI test of [152](152-three-signals-end-a-turn.md), with the
  `sleep_tool` added there.
- **2026-10-05** — Done, waiting on the token around the call. `GuardedToolExecutor::execute` takes
  the turn's `CancellationToken` as an argument - like the streaming loop and the wait for
  approvals, it comes from the agent's `RunControl`, not from `Turn` - and `select!`s the
  executor's future against it, biased so that a tool done at the same moment keeps its result. On
  the token the future is dropped and the call answered with the new status `Interrupted` (model,
  web UI), with `started_at` and, as `finished_at`, when it was interrupted: it counts as having
  run, and a turn given up tells the agent of it. `handle_executing_tools` starts no further call of
  the step; those left are abandoned as not executed when the turn ends.

  Decided: a cancel interrupts a tool as a stop does - as it already interrupts a model call and the
  wait for an approval - so that a hanging tool no longer blocks the stop button. A tool that must
  not be cut off would say so; none needs it yet. Tools do not see the token (the cooperative
  variant): one that needs to clean up when interrupted cannot yet, which is left until one does.
  Documented in TOOLS-AND-COMMANDS.md (a tool must expect to be dropped at any `await`),
  AGENT-TURN.md and CHANNELS.md.

  Not changed: after a crash during `ExecutingTools`, `prepare_resume` still answers an approved
  call without a result as "not executed: the server restarted before it ran", although it may
  have run; `Interrupted` would describe it more honestly. Running the calls of a step concurrently
  was discussed alongside and split off as [226](226-concurrent-tool-calls.md).
- **2026-10-05** — Addendum: what dropping a tool stops, and what it does not. TOOLS-AND-COMMANDS.md
  now says what a tool has to do so that dropping it stops it - bind spawned tasks and child
  processes to its future, never block - and what happens to an interrupted MCP call. A guard in
  `mcp_tool` that sends `notifications/cancelled` for the dropped request was considered and left
  out: rmcp only cancels a request it was handed as a `RequestHandle`, which `call_tool` - with its
  rounds for `input_required` - does not give out, so the guard would mean rebuilding that loop.
  And it would add little: each call has an MCP session of its own, which rmcp aborts and deletes
  when the dropped connection is closed.
