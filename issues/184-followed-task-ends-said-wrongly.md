# 184 — A followed task's failure or broken stream is said wrongly

**Status:** done
**Area:** Integrations
**Summary:** When a task an external agent follows fails or its stream breaks, the agent records a message nobody saw or publishes a half answer.

## The problem

Two cases in which `ExternalAgentActor` handles the end of a task it follows between requests
(issue [179](179-external-agents-via-a2a.md)) differently from what its comments and the request
path do:

- **A failed task leaves a message nobody saw.** When the remote task fails after some text came,
  `speak` appends that text to the agent's history and stores it, but publishes only `TurnFailed`.
  The history then holds a message the channel never saw, and what the remote agent is given next
  (`unheard`) is counted against it. `exchange` returns on `Failed` without recording anything.
- **A broken stream publishes a half answer.** On `Some(Err(_))` in `on_followed` the comment says
  the task stays open to be followed again, but the code falls through to `speak`: what came so far
  is published as the agent's finished message while the task is still working - and may be said
  again in full when the task is followed on the next run.

## Why it matters

The channel and the agent's own record disagree about what the agent said, and an answer can appear
twice, once cut off.

## Considerations

- A failure records nothing in the history, as on the request path; a broken stream keeps what came
  pending (or drops it) and says nothing until the task ends.
- Easiest after issue [183](183-external-events-folded-twice.md), which leaves one place for both.

## Done when

- A failed followed task publishes `TurnFailed` and records no message; a broken stream publishes
  nothing; both are covered by tests.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series; both cases confirmed in
  `external_agent.rs` (`speak`, `on_followed`).
- **2026-10-02** — Done. `speak` takes the outcome of the task, an answer or the reason it failed;
  a failure publishes `TurnFailed` and records nothing, as on the request path. `on_followed` says
  nothing on a broken stream: what came so far waits for what may still come, and is dropped
  without a word when the stream ends before the task did - the same half answer reached `speak`
  through the stream's end, which follows the error. The task stays open and is followed again on
  the next run. Tests: `a_followed_task_that_fails_says_so_and_keeps_nothing_of_what_came_before`,
  `a_followed_task_whose_stream_breaks_says_nothing_and_stays_open`. Not covered by a test: a
  stream that ends without an error before its task ends - the scripted remote agent never ends
  its streams.
