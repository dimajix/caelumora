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

Two more calls are made and not counted. A summary whose text is empty is no summary: `ask`
answers the message, `replace` filters it away and fails with `NoSummary`, and the tokens of its
call go with it - the turn is given up, but the call was made. And an `EmitOutProcessor` may drop
a message of the agent's model (issue 196), and with it its call; none does so today.

Some calls cannot be counted however it is decided: a call cancelled (`AgentActor` drops its
stream), one whose stream fails midway, the question after a summary when it fails. A model reports
its measures with the end of its answer - Ollama in its last chunk -, so a call cut short reports
none. `TokenUsage::calls` already says so: it counts the calls that reported their tokens.

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
    remote agent's calls, the retries of a client - is open. And `TokenUsage::counted` sets both
    groups of the usage at once: it adds to what was processed, and sets the context - its
    `context_tokens`, its `context_size` - to the call's. The context of an implicit call is not
    the agent's, so counting one needs a way that adds to the usage alone, a third beside
    `counted` and `summed`. For a compaction it does no harm: `AgentChannelState::compact`
    replaces the context by the estimate afterwards.
*   **The measures of a call named**: what a call reports - its tokens, its durations, the size of
    its context - is a concept of its own, today only a convention of six fields of `Message`.
    Named, say `CallMeasures`, a message carries those of its call, `TokenUsage::counted` counts
    measures rather than a message, and `Replacement` carries the measures of each call the
    strategy made, which `compact` counts one by one; the summary's message keeps those of its own
    call, and `summed` goes. The next implicit call hands its measures to whoever counts them in
    the same way, and where counting ends has a plain answer: the calls of the agent's own
    `LanguageModelClient`. Whether a call also sets the context is decided by whoever counts it,
    not by its measures. It is the previous option with a name instead of a way of its own; it
    changes `Message`, and with it what is stored.
*   **The strategy answers its calls apart**: `Replacement` carries the answer of each call, and
    `AgentChannelState::compact` counts them apart from the messages it records. Fixes the count
    for compaction alone; the next implicit call needs the same again.
*   **As it is**, the exception named where the rule is stated (AGENT-TURN.md *Tokens*,
    `TokenUsage::calls`, `AgentChannelState::used`).

Not decided yet: the maintainer wants to see whether further cases of implicit calls appear
before choosing.

## Done when

*   It is decided how a call that produces no message of its own is counted, and the usage counts
    every call made that reported its measures, its tokens and durations, as decided.

## History

- **2026-10-30** — Opened while finishing issue 413, whose question after the summary is the
  second call of a compaction; its tokens are summed into the summary's message for now.
- **2026-11-03** — Looked again for further implicit calls: none has appeared. The agent's model
  is still asked in two places alone - a turn's call (`AgentActor`) and a compaction by summary
  (`SummaryCompaction::ask`) -; the decision model, the image models and external agents report no
  measures. Added what was found on the way: the empty summary and the dropped message, the calls
  that report nothing, that `counted` sets the context as well, and the option of naming a call's
  measures. Still waiting for the decision.
