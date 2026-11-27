# 622 — A compaction summarises the brief a sub-agent works on

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** A compaction replaces the brief an agent was delegated - the task it works on and the framing that tells it that only its answer goes back - with what its summary keeps of it, while it carries the parts of prompt components over as they stand, so a sub-agent working long on a brief may lose its task's wording and how to answer it.

## The problem

Since issue [619](619-a-sub-agent-gives-back-everything-it-said.md), a brief a thread's delegate
gives (issue [557](557-a-conversation-cannot-go-on-two-ways-at-once.md)) is a `Given` entry of the
recipient's transcript of its own kind (`GivenSource::Brief`), framed for the model as one
(`kernel.given_brief`): who gave it, the brief, and that only the agent's answer to it - its last
message, calling no tool - goes back and is all the giver reads of its work. Before 619 the
terminal's helper said the second part in its system prompt.

A compaction (AGENT-TURN.md *Compaction*, `caelumora-compaction/src/cut.rs`) treats what lies
before its cut in three ways:

- **parts of prompt components** given into the transcript are carried over as they stand
  (`Transcript::carried_parts`, issue 537);
- **knowledge** read whole - a skill, a file - that the part kept does not know still is given
  again (issue 413);
- **everything else** is replaced by the strategy's summary.

A brief is the third kind. It is given before the turn that answers it begins, so every cut of
that turn lies after it: a sub-agent that works long enough on its brief to have its context
compacted - reading files, running commands - has its brief replaced by what the summary keeps of
it. The summary is asked to keep what the task is and who asked for it; whether it keeps the
brief's wording and the framing - that the answer is all the giver reads, complete and concise - is
the model's to decide. After the cut, nothing tells the sub-agent so any more.

The task itself was summarised so before 619 as well; 619 moved the framing into the brief, so now
both go the same way.

## Why it matters

- A sub-agent's work on one brief is exactly the long, tool-heavy work that fills a context - the
  point of a sub-agent (issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md)) is that the main
  agent does not do it itself.
- What the giver asked for, in its words, is what the answer is measured by; a summary of it is a
  second-hand task.
- Without the framing, the answer after a compaction is what 619 set out to avoid: written without
  knowing that it is all the giver reads.

## Considerations

- **A. The open brief is carried over, as parts are.** The entry of the brief the turn under way
  answers - the last `Given { source: Brief }` on the branch - counts as kept wherever the cut lies,
  and is copied after the replacement as it stands, as `Transcript::carried_parts` does for parts.
  The cut, the strategy and `keep` count it as they count a part. Briefs answered before are history
  and are summarised as anything else. Recommended, as the maintainer decided on 2026-11-27 in issue
  619.
- **B. The framing in the personality as well.** The helper's prompt says it again; documented as a
  gap for every other personality. Cheap, but the task is still summarised, and the framing is said
  twice.
- **C. The brief given again**, as knowledge is: given anew after the replacement, by the delegate
  as giver. A second `Given` entry of the same brief, and a brief is no knowledge read whole.

Open: whether "the open brief" is read from the transcript alone - the last brief on the branch -
or from the turn (the brief whose request began it), and how a compaction between turns
(`/compact`) treats it.

## Done when

- A compaction of a turn answering a brief keeps the brief as it stands, framed as one, whatever
  the cut; earlier briefs are replaced as before.
- Covered by tests of the cut and of `Transcript::compact`; AGENT-TURN.md (*Compaction*) says it.

## History

- **2026-11-27** — Opened from the review of step 3 of issue 619, as the maintainer decided:
  approach A.
- **2026-11-27** — Decided by the maintainer: the open brief is read from the transcript alone -
  the last brief on the branch after which no turn ended, as the delegate takes the first turn
  that ends after a brief as its answer -, so that `/compact` between turns replaces a brief
  answered; and the strategy is not given it, as it is given no part, since the replacement is
  classified by what it replaces. Planned in two steps: what a compaction carries over named once
  (`Transcript::carried_over`), then the brief under way added to it.
- **2026-11-27** — Step 1 done: `Transcript::carried_parts` became `Transcript::carried_over`, and
  what a summary is written of leaves out every part and what a compaction carries over.
- **2026-11-27** — Step 2 done: the brief under way is carried over (`brief_under_way`). A
  command turn ends it as well, though the delegate takes it for no answer; it comes between a
  brief and its turn only if a command is asked for before that turn begins, and is left so.
- **2026-11-27** — Done: both steps reviewed and accepted by the maintainer; the instruction of
  the summary stays as it is, since it holds for an agent without a brief.
