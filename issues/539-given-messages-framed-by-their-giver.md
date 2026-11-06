# 539 — What is given into an agent's transcript is framed by whoever gives it

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** What is given into an agent's transcript - a skill, a compaction's summary, a prompt part - takes the role and sender its giver chooses, so the model cannot tell given context from what was said, and a skill a user loaded is given again after a compaction as the agent's own.

## The problem

An entry `TranscriptEntry::Given` is a `Message` its giver builds, with a role and a sender of its
choosing:

- a skill the model loads (`load_skill`, `caelumora-skills/src/component/tool.rs`): role `User`,
  sent by the agent itself, so unmarked;
- a skill a user loads (`/skill`, `caelumora-skills/src/component/command.rs`): role `User`, sent
  by that user, so marked as their message - deliberately, as their act;
- a compaction's summary (`caelumora-compaction/src/compactor.rs`): role `User`, sent by the
  agent, unmarked, recognisable only by its title in brackets;
- a skill given again after a compaction (same file): sent by the agent, whoever loaded it - a
  skill a user loaded loses that it was theirs;
- a prompt part at `Given` (issue [537](537-prompt-parts-placed-anew-every-turn.md)): a system
  message of its component, where the model keeps one (`SystemMessages`), else a user message.

The channel's notices of who joined or left, by contrast, are worded and framed by the
`ContextResolver` alone, and the profile's `framing_explanation` tells the model how others'
messages are marked.

## Why it matters

- **A model cannot tell context from conversation.** A summary or a skill given as an unmarked
  user message reads like something the agent said to itself; Claude Code marks everything its
  harness inserts the same way (`<system-reminder>`) and explains that once in its system prompt.
- **Who gave a skill is lost** at the first compaction, though the transcript recorded it.
- **Every giver repeats the choice**, and a new one - the files of issue
  [538](538-workspace-knowledge-without-a-place.md) - would choose again.

## Considerations

- **The resolver frames what is given**, as it frames the notices: a system message where the
  profile keeps one, else a user message marked as the kernel's, the marking explained once in
  `framing_explanation`. A giver says what it gives and as whose act, not how it reads.
- **A user's act stays theirs.** A skill a user loaded is framed as given at their request; the
  compaction gives it again with the same origin. The origin belongs to the entry (`GivenSource`
  or the message's sender), not to the restorer.
- **The parts at `Given`** already take the role the profile allows; they would take the common
  frame too, so that one rule serves all.
- **Experiments.** Whether small models follow a marked user message as well as an unmarked one
  is to be tried, as `work/quiet_agents` tried the mentions.

## Done when

- Everything given into a transcript is framed by the `ContextResolver` by one rule, explained to
  the model once.
- A skill a user loaded is given again after a compaction as theirs.

## History

- **2026-11-06** — Opened from the comparison of Claude Code's session logs with Caelumora's
  concepts that led to issue 537 (point C there).
