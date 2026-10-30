# 514 — A model call that produces no message of its own is counted with another

**Status:** open
**Type:** question
**Priority:** low
**Area:** Observability and operations
**Summary:** The statistics of a model call - its tokens and durations - are recorded on the message it produced, so a call whose answer is no message of the transcript is counted with another message, or not at all, and `TokenUsage::calls` counts fewer calls than were made.

## The problem

Every message an agent's model produces carries the measures of its call - `prompt_tokens`,
`completion_tokens`, `context_size`, the durations -, and `AgentChannelState::used` sums them as
the messages are recorded, each call counted once (issue [209](209-token-usage.md),
[AGENT-TURN.md](../docs/AGENT-TURN.md), *Tokens*). The measures belong to the message itself, and
that is meant: a message's duration is that of its call, not the wall time of what came with it -
the tool calls of its step, say.

Not every call produces a message of the transcript. A compaction by summary (issue
[284](284-automatic-context-compaction.md)) makes a call whose answer is not recorded as the
model's: its text becomes the summary, a message given to the agent, which carries the call's
measures. Since issue [413](413-compaction-restores-unused-skills.md) it makes a second call after
the summary - which of what can be given again the work still needs -, whose answer is no message
at all. Its tokens are added to those of the summary (`summary_compaction::summed`); its durations
are dropped, and `TokenUsage::calls` counts one call for both. The web UI shows that number as
"Usage of all N calls".

## Why it matters

*   The number of calls shown is too low, and the durations of a compaction are incomplete.
*   The rule "a call is counted once, as the message it produced" holds only where every call
    produces a message; each further implicit call bends it again in its own way.
*   `summed` is a second way of adding up measures, beside `TokenUsage::counted`.

## Considerations

*   **A second set of statistics for implicit calls**: measures recorded apart from the messages,
    for the calls the agent made on its own behalf - a compaction, a question to its model -, and
    summed into the usage beside those of the messages. Keeps the measures of a message its own;
    but it is a second concept beside the message's, and where it ends - a classification model, a
    remote agent's calls, the retries of a client - is open.
*   **The strategy answers its calls apart**: `Replacement` carries the answer of each call, and
    `AgentChannelState::compact` counts them apart from the messages it records. Fixes the count
    for compaction alone; the next implicit call needs the same again.
*   **As it is**, the exception named where the rule is stated (AGENT-TURN.md *Tokens*,
    `TokenUsage::calls`, `AgentChannelState::used`).

Not decided yet: the maintainer wants to see whether further cases of implicit calls appear
before choosing.

## Done when

*   It is decided how a call that produces no message of its own is counted, and the usage counts
    every call made, its tokens and durations, as decided.

## History

- **2026-10-30** — Opened while finishing issue 413, whose question after the summary is the
  second call of a compaction; its tokens are summed into the summary's message for now.
