# 514 — A model call that produces no message of its own is counted with another

**Status:** done
**Type:** bug
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

## Decision

Decided by the maintainer on 2026-11-25, after no further implicit call had appeared since issue
413: **the measures of a call named**, the second option above.

*   `CallMeasures` is what a call of a language model reports of itself - its tokens, its
    durations, the size of its context -, and `Message::measures: Option<CallMeasures>` replaces
    the six fields of `Message`: `None` for a message no call produced. A model that reports a
    size and no tokens made no call counted: `TokenUsage::counted` counts measures, and keeps its
    check for reported tokens.
*   **Nested, not flattened, in what is stored and sent.** `#[serde(flatten)]` would keep the JSON
    as it was, but works only with a `CallMeasures` every message has, empty where no call
    produced it, which says less than an `Option`. Stored data may be deleted (CODING-GUIDELINES,
    *Stored data*), and no column of the Postgres store holds a measure: messages are kept as
    `jsonb` in the transcripts and histories. A table of calls with columns of their own would be
    the place for an evaluation across channels, not columns of the messages; nobody needs one
    today.
*   **The summary's message keeps the measures of its own call**, durations included; the
    strategy's further calls are carried apart (`Replacement::further_calls`) and counted by
    `AgentChannelState::compact`, which counts what carries measures - no longer what its agent
    sent. `counted` sets the context too, which does no harm there: the estimate of the new
    branch replaces it at once. `summed` goes.
*   **Left as they are, named where the rule is stated:** the call of an empty summary, whose
    turn is given up and which no `Replacement` carries; a message an `EmitOutProcessor` would
    drop, which none does; the durations of the further calls, since `TokenUsage` sums no
    durations of any call and nobody needs a sum of them.

## Plan

Two steps; the pattern is `TokenUsage` beside `Message`, a data type of the domain, and calls are
counted where they are today, in `AgentChannelState::said` and `compact`, by `TokenUsage::counted`.

1.  **The measures named, without a change of behaviour.** `CallMeasures`, `Message::measures`,
    `TokenUsage::counted(usage: Option<Self>, measures: &CallMeasures) -> Option<Self>`; the
    Ollama client builds the measures with the end of its answer and adds the size of the context
    to them; `Replacement::measures`, an `Option<CallMeasures>`, instead of `Replacement::call`, a
    `Message`, and `summed` adding measures for now; the web UI reads `Message.measures`.
2.  **Every call counted.** `Replacement::further_calls: Vec<CallMeasures>`, passed through
    `Turn::compact` to `AgentChannelState::compact`, which counts the measures of every message
    of the replacement and then each further call; `summed` and the check of the sender go;
    AGENT-TURN.md *Tokens used* names the exceptions.

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
- **2026-11-25** — Decided: the measures of a call named (`CallMeasures`), nested in the JSON;
  the plan in two steps above. Step 1 done: `CallMeasures` and `Message::measures` replace the
  six fields of `Message`, `TokenUsage::counted` counts measures, `Replacement` carries measures
  instead of a message. A summary's message now carries the durations of its call where no
  question follows it; where one does, the tokens of both are summed without durations.
- **2026-11-25** — Step 2 done: `Replacement::further_calls` carries the measures of the
  strategy's calls whose answers stand in no message - `summary_compaction`'s question after its
  summary -, `Turn::compact` passes them to `AgentChannelState::compact`, which counts them after
  the measures the replacement's messages carry, whoever sent them; `summed` is gone. The
  exceptions are named in AGENT-TURN.md *Tokens used*, at `TokenUsage::calls` and at
  `EmitOutProcessor`. The estimate replaces how full the context is, not its size: that is the
  last call's counted, the question's, as it is after every call. Done.
