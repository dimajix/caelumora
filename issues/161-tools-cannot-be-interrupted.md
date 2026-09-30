# 161 — A running tool cannot be interrupted, so a stop during it aborts the agent

**Status:** open
**Area:** Tools and approvals

## The problem

`ToolProvider::execute` and `ToolExecutor::execute` get the turn, not the
turn's `CancellationToken`, and nothing around them waits on it. A tool
therefore runs to its end, whatever happens meanwhile:

- **A cancel** waits for the tool. The turn ends only after it - correct,
  but a tool that runs for minutes (an MCP server that hangs) keeps the
  stop button red for minutes.
- **A stop** - the agent removed from the channel, or the channel deleted -
  also waits, but the channel does not: after `STOP_GRACE` (5 s,
  `channel.rs`) it aborts the actor's task. The turn is then neither given
  up nor taken over. What is stored is the state from before the tool ran
  (`ExecutingTools`, the call approved, no result). When the agent joins
  again, `prepare_resume` answers that call as "not executed" - although it
  ran, perhaps to its effect - and no dead branch records it, against what
  [145](145-ended-turns-leave-a-record.md) promises for every stop.

Found in the web UI while testing [152](152-three-signals-end-a-turn.md):
removing an agent while its `sleep` tool (30 s) ran logged "did not stop by
itself … aborted" after 5 s.

## Why it matters

- The record of what a tool did depends on how long the tool takes.
- A hanging tool blocks cancelling for as long as it hangs.

## Considerations

- **Wait on the token around the call.** `GuardedToolExecutor::execute`
  `select!`s the executor's future against the turn's token; on the token
  the future is dropped and the call answered as interrupted, with the
  `started_at` it has - so the call is recorded as having run, and the
  turn can be given up or taken over. Dropping a future is not undoing its
  effect: an HTTP request already sent is sent ([20](20-transactional-effects.md)).
  The executor would need the token - through the turn, or as an argument.
- **Only for a stop, not for a cancel.** A cancel could keep waiting (the
  user asked to end the turn, not to break a tool off halfway); a stop
  cannot wait longer than the grace. Or both interrupt, and a tool that
  must not be cut off says so.
- **A longer grace** only moves the line.

## Done when

- A stop while a tool runs leaves the turn given up, with the call
  recorded as having started, however long the tool takes.
- Whether a cancel interrupts a tool is decided and documented.

## History

- **2026-09-30** — Found in the UI test of
  [152](152-three-signals-end-a-turn.md), with the `sleep_tool` added there.
