# 413 — A compaction gives again every skill the agent once loaded

**Status:** done
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

## Decision

Decided on 2026-10-30. A skill is in an agent's context three ways: that it exists - the catalogue,
which no compaction touches -, that the work under way follows it, and its text. Whether the work
follows it is a fact about the task, and the one who knows the task when the context is compacted is
the model that summarises it. So the strategy asks its model, after the summary, which of what was
given in the part replaced it still needs; what it no longer needs is not given again, and the
owners are told so apart from what can no longer be given. Whether something is still needed does
not depend on its kind: the restorers stay as they are, and answer only what they can still give
and as what.

No trait of its own decides it (a `KnowledgeSelector` was weighed): what the agent still needs is,
with the summary, the strategy's answer to what replaces the part, and the only implementation would
be asking the model - "all" is no more than the fallback where its answer cannot be read.

The question is asked in the conversation of the summary, after it: the same prefix, so a model
that caches it reads it once, and an answer apart from the summary, matched against the names asked
about. An answer that cannot be read gives everything again, as before.

### The strategy apart from the mechanics

`SummaryCompaction` holds two things today: the rules every compaction keeps - where the transcript
may be cut, what was given in the part replaced, whose the replacement is and how it is classified,
what is given again and how the context moves - and what is particular to a summary - the model, its
instruction, the summary's text. The rules of confidentiality and of the transcript's integrity do
not belong to what can be exchanged. Split first, as a step of its own without a change of
behaviour, in the crate `caelumora-compaction`:

*   **`cut.rs`**, `Cut`: where an agent's transcript is cut for a compaction - the part replaced and
    the part kept -, what was given in the one and not in the other, and whose the replacement is
    and how it is classified. Plain functions of entries and positions: `cut_at`, `kept_whole`,
    `given_in`.
*   **`compactor.rs`**, `Compactor`: a compaction carried out - the transcript cut, the part
    replaced by what the strategy puts there, what was given there given again by the restorers,
    the context moved (`Turn::compact`), what was done told as a `Compaction`. Holds the strategy,
    the restorers and `keep`.
*   **`component.rs`**: `compaction` holds the `Compactor` and takes `keep` beside `threshold`,
    both shares of the context size that depend on each other - `keep` below `threshold`, checked
    as the component is built, where until now a document asked for it.
*   **`summary_compaction.rs`**: the strategy alone - the model, the instruction, the summary.
*   **`CompactionStrategy`** answers only what replaces the part:

    ```rust
    async fn replace(&self, turn: &Turn<'_>, part: Vec<Message>, given: &[String])
        -> Result<Replacement, CompactionError>;
    ```

    `part` is the part replaced as the model is given it, `given` names - by kind and name, as
    `/compact` tells them - what was given in it and is given again. `Replacement` holds the text
    and the message of its model's call, for its measures; with step 2 `still_needed`, which of
    `given` the agent still needs.

### Plan

1.  The split above, without a change of behaviour but one: the instruction of the summary names
    what is given again, by kind and name, where it named the kinds the restorers give - the
    strategy knows no kind. The tests of the cut move to `cut.rs`, those of a compaction carried out
    to `compactor.rs` with a strategy that answers a fixed text, those of the summary stay with
    `summary_compaction.rs`. Documents: AGENT-TURN.md (*Compaction*), COMPONENTS.md (`keep` moves
    to `compaction`), SKILLS.md.
2.  The question of what is still needed: `Replacement::still_needed`, a second text of the
    catalogue, its answer matched against `given`, everything given again where it cannot be read;
    `Compaction` tells what was not needed apart from what is lost; the instruction of the summary
    says that what is given again is only given again if still needed.

## History

- **2026-10-17** — Opened after the compaction in session `s2` gave Jonathan the quiz skill again in
  the middle of translating a program.
- **2026-10-30** — Discussed and decided, see *Decision*: the strategy asks its model what is still
  needed, no trait of its own; first the mechanics are split from the strategy. Branch
  `issue-413-compaction-restores-unused-skills`.
- **2026-10-30** — Step 1 done: `cut.rs` (`Cut`), `compactor.rs` (`Compactor`), `keep` with the
  `compaction` component and checked below `threshold`, `summary_compaction` the strategy alone
  (`CompactionStrategy::replace`, `Replacement`). `still_needed` waits for step 2, which is its
  first user. A test through the runtime (`caelumora-skills/tests/skills.rs`) shows the restorers
  reaching the compaction: a skill given with `/skill`, then `/compact`.
- **2026-10-30** — Step 2 done: `summary_compaction` asks its model after the summary which of what
  can be given again the work still open needs - a line a name, or "none" -, and `Replacement`
  answers it as `still_needed`; an answer that names nothing and does not say "none", an empty one
  or a model that cannot be asked gives all of it again. The tokens of the question count with those
  of the summary, summed into its message - how a call without a message of its own is counted is
  issue [514](514-calls-without-a-message-of-their-own.md). `Compaction::not_needed` tells what was
  not given again, apart from what is lost. Tests: the strategy (`summary_compaction.rs`), the
  compactor (`compactor.rs`), and through the runtime two skills given, the one still needed given
  again (`caelumora-skills/tests/skills.rs`).
