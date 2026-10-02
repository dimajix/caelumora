# 183 — An external agent folds the remote agent's events twice

**Status:** done
**Area:** Integrations

## The problem

`ExternalAgentActor` (issue [179](179-external-agents-via-a2a.md)) takes in what the remote agent
sends in two places, which do the same with small differences that have no stated reason:

- **Two folds over `ExternalEvent`.** `exchange` (answering a request) and `on_followed` (a task
  followed between requests) each `match` every event kind: the conversation is recorded, whole
  messages and a question back are collected into an `Answer`, `InputRequired` marks the task as
  asking back, `Completed`/`Canceled` and `Failed` close it.
- **Two ways to publish an answer.** `respond` and `speak` each build the message with the
  classification fallback `remote.classification().unwrap_or_else(|| channel.classification())`,
  append it to the history, store the state and send `CompleteMessageGenerated`,
  `FullResponseGenerated` or `TurnFailed`.

The copies already differ: `Partial` is published while answering a request but only collected
while following a task; the failure reads "could not answer" in one and "could not finish" in the
other; a stream error ends a request but leaves a followed task open. Two bugs of the followed path
(issue [184](184-followed-task-ends-said-wrongly.md)) sit exactly in the copy that differs.

## Why it matters

Every new event kind or task state - a classification sent by the remote agent (the A2A sibling of
issue [139](139-mcp-result-classification.md)), `AUTH_REQUIRED` handled other than as a failure -
has to be added twice, and the copies drift further.

## Considerations

- **One fold** that takes an event into the state and the `Answer` and says what it means: go on,
  asked back, ended, failed. `exchange` adds what only it does (publishing partials, tracking the
  task it started), `on_followed` turns the outcome into `speak`.
- **One publication** of an ending, used by both; the only intended difference is the leading
  `WakeupRequested` of a message the agent says on its own account.
- **Restructure, then fix:** the refactoring is a step of its own without behaviour changes; the
  differences that turn out to be bugs are fixed afterwards under issue 184.

## Done when

- `ExternalEvent` is matched in one place, and an ending is published in one place.
- Remaining differences between answering a request and following a task are named in the code.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series found the two folds
  independently from three angles (reuse, quality, altitude).
- **2026-10-02** — Done, without behaviour changes. `Answer::take` takes an event into the answer
  and the state and says what it comes to (`Step`); `exchange` adds the task it starts and the
  partials it shows, `on_followed` waits for the end. `record` builds the message and keeps it in
  the history, `answered` gives the events that publish it, `Answering::send_all` sends them, and
  `classification_in` says how an answer is classified - each used by both paths. Left as they
  were: the failure reads "could not answer" for a request and "could not finish" for a followed
  task, and a broken stream of a followed task still ends in `speak` - issue
  [184](184-followed-task-ends-said-wrongly.md).
