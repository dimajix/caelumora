# 413 — A compaction gives again every skill the agent once loaded

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Context and knowledge
**Summary:** After a compaction, the `skills` component gives the agent again every skill loaded in the part that was summarised, whether the work still needs it or not, so a context just made smaller is filled with instructions from an earlier task.

## The problem

When the context is compacted (issue [284](284-automatic-context-compaction.md)), what was given in
the summarised part is given again after the summary by the component that gave it - a
`KnowledgeRestorer` ([AGENT-TURN.md](../docs/AGENT-TURN.md#compaction)). The `skills` component
gives every skill whose `Given` entry lies in that part and that the agent may still load.

In session `s2` on 2026-10-17, Jonathan had loaded the skill `quiz-round` for a quiz early in the
channel. Much later, while translating a program to Node.js, his context was compacted, and right
after the summary he was given "[Skill: quiz-round] 1. Announce the topic and how many questions the
round has …" again - instructions for a task long over, in the middle of another, at the moment the
model is most easily confused (see [411](411-call-limit-ends-turn-silently.md)).

## Why it matters

*   It takes back part of the room the compaction made, and every skill loaded once comes back at
    every compaction, so over a long channel they add up.
*   A skill read as an instruction - "ask one question at a time" - can pull the model towards the
    old task.

## Considerations

*   **Only what the summary still needs.** The summary says what the work is now; a skill whose
    task is over is not given again. Who decides: the strategy, by asking the summarising model
    which of the given entities are still needed, or the restorer, by when the skill was last used.
*   **Only what was loaded in the kept part's task.** Simple, but a skill loaded at the start of a
    long task is exactly one that is still needed.
*   **Name instead of body.** Give again only that the skill was loaded, by name, and let the agent
    load it again if it needs it (`load_skill`). Small, and the model decides; but a skill that
    should guide the rest of a task is then easily forgotten.

Recommended to weigh the first against the third.

## Done when

*   After a compaction, a skill of a task that is over is not given again in full, and one the
    current task uses still is; a test covers both.

## History

- **2026-10-17** — Opened after the compaction in session `s2` gave Jonathan the quiz skill again in
  the middle of translating a program.
