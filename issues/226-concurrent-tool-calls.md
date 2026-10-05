# 226 — The tool calls of one step run one after the other

**Status:** open
**Area:** Tools and approvals

## The problem

When the model makes several tool calls in one answer, `handle_executing_tools` runs them one after
the other: each `GuardedToolExecutor::execute` is awaited before the next begins. Three calls of
slow tools - MCP servers, remote lookups - take the sum of their durations instead of the longest.

They cannot simply be run concurrently: every tool is handed `&mut Turn`, and several futures
cannot hold it at once. The tools write into the turn while they run:

| Tool | writes |
|---|---|
| `notes`, `channel_history` | `record_knowledge` |
| `create_plan` | `take_plan`, `set_plan` |
| `stop` | `request_state` |
| `GuardedToolExecutor`, admitting a result | raises the classification the turn has seen |

The other tools only read `label`, `session` and `security_context`.

[TOOLS-AND-COMMANDS.md](../docs/TOOLS-AND-COMMANDS.md#running-tool-calls) says another way of
running tools "in parallel" can be tried by configuring a different `ToolExecutor`. It cannot:
`ToolExecutor::execute` is called once per call, and the loop over the calls is the agent's.

## Why it matters

- The latency of a step with several slow calls is the sum of the calls.
- The more remote tools an agent has, the more this costs.

## Considerations

- **A `Mutex` around the turn** would be held across `await`s, and the order of the writes would be
  whatever the scheduler makes of it. A workaround, not a structure.
- **Tools read the turn and return their effects.** A tool gets a read-only view of the turn and
  collects what it changes in a value of its own per call (knowledge read, plan, requested state).
  The runtime applies these in the order of the calls once the step is done - or drops them, for a
  call that was interrupted ([161](161-tools-cannot-be-interrupted.md)), which then can no longer
  leave the turn half changed. This changes the signature of `ToolProvider::execute` and touches
  every provider. It is a refactoring of its own, without behaviour changes, before the calls run
  concurrently - and an architecture decision.
- **Order of calls.** Today calls of one answer happen to run in order, which a local model that
  writes a note and reads it back in the same answer relies on without knowing it. The common model
  APIs treat the calls of one answer as independent. Whether running them concurrently needs to be
  switchable - per tool, per personality - is open.
- **Classification.** Today the result of the first call raises the classification before the second
  is admitted; concurrently, every call is judged against the state before the step and the
  classifications are merged afterwards. That is arguably more consistent (`create_plan` already
  captures `progress_before` for the same reason), but it is a change to what is checked and needs
  its test in the security suite.
- **Storing results.** Results come in any order. Storing each one as it arrives, rather than at the
  end of the step, keeps a crash from losing those that are in.
- **Who decides.** The loop over the calls is the agent's, the way of running one call the
  executor's. Running concurrently could stay the agent's loop (`join_all` of the guarded calls,
  `select!`ed against the turn's token as each call already is), or the `ToolExecutor` could be
  handed the step's calls - which would make the documented claim true, at the price of a wider
  facet.

## Done when

- The calls of one step run concurrently, and the step takes as long as its longest call.
- A cancel or stop interrupts all of them, each answered as interrupted.
- Tools no longer hold `&mut Turn` while they run.
- TOOLS-AND-COMMANDS.md says correctly where running in parallel is decided.

## History

- **2026-10-05** — Split off from [161](161-tools-cannot-be-interrupted.md), where running the calls
  concurrently was asked about as a way of waiting on all of them and the token at once. Waiting on
  the token needs no concurrency, so 161 was done with the calls in order; this records what
  concurrency would cost.
