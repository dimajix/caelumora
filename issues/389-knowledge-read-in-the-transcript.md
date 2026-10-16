# 389 — A compaction reads from the history what it gives the model again

**Status:** open
**Area:** Context and knowledge

## The problem

A compaction gives the agent again what it had read in the part it summarises - a skill, any entity
of a kind a `KnowledgeRestorer` serves - unless it is still there in the part kept
(`SummaryCompaction`, issue [284](284-automatic-context-compaction.md)). What was given is read
from two places:

- what was given as a message, from the transcript: `TranscriptEntry::Given { source }`;
- what a turn read otherwise - recorded as knowledge with what it said afterwards (`load_skill`,
  say, through `Turn::record_knowledge`) - from the history: `given_read` collects the
  `KnowledgeSource::Entity` of every `ContextRef` the history recorded with a message.

The second reads the history to build what the model is given. The history of an agent is the
record of what was said and what it was produced from, for people to read; what the model is given
is built from the transcript alone. The transcript does not hold what a turn read through a tool or
a prompt part, so the compaction has nowhere else to find it.

## Why it matters

- It breaks the rule that the model's input is built from the transcript alone: the history is
  informational and may be held only in part
  ([387](387-agent-history-window-without-compaction.md)). A history held in part would give again
  less than was read, without anyone noticing.
- What was read is a property of the transcript's branch - it is what the model was shown there -
  and is found today only by going through a second chain by message id.

## Considerations

- **Where it is recorded.** The transcript records what was read where it was read. Either as part
  of the entry of the message it came with - a field not covered by the digest, as
  `Given::source` is, since the model is given the message alone - or as an entry of its own,
  which gives the model nothing, like a turn's end.
- **When it is recorded.** Knowledge is recorded on the turn's context when it is read
  (`AgentWork::record_knowledge`), and with every message said afterwards; the transcript would
  record it once, at the point it was read or with the next message said.
- **A compaction and its copies.** What the compaction gives again goes into the new branch as
  `Given`; what the part kept read must be kept with it, so that a later compaction finds it.
- **Which knowledge.** Only knowledge of an entity is given again (`KnowledgeSource::Entity`); the
  rest - notes, memory, another channel's history - is recorded on the context for confidentiality
  and provenance (issues 17, 61) and need not be in the transcript for this.

## Done when

- `SummaryCompaction` finds what was read in the transcript alone; `given_read` is gone.
- AGENT-TURN.md says where the transcript records what was read.

## History

- **2026-10-16** — Opened from the discussion of
  [387](387-agent-history-window-without-compaction.md): the history is informational and never
  used to build the model's input; this is one of the two places that do it today.
