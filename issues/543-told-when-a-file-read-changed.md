# 543 — The agent is not told that a file it read has changed since

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Context and knowledge
**Summary:** A file the agent read and somebody else changed since is refused to be written over, but the agent learns of the change only when it tries to write, and answers and plans from what it read until then.

## The problem

Since issue [538](538-workspace-knowledge-without-a-place.md), what an agent read of a workspace
is known by its place and digest, and `write_file` and `replace_in_file` refuse to change a file
the agent last knew otherwise (`AgentChannelState::last_known`): nobody's work is overwritten
unnoticed. But the agent learns that the file changed only then. Until it writes, it answers from
what it read - "config.yaml sets X", which no longer holds - and plans changes to lines that are
gone; its write is refused perhaps after a person approved the call.

## Why it matters

- **Answers from a stale file**, given as if they held.
- **Work planned in vain**, and an approval asked for a change that cannot be made.
- A workspace is used by several channels and people at once, so a file changing under an agent
  is no exception.

## Considerations

- **A prompt part of `workspace` at `Given`** (issue
  [537](537-prompt-parts-placed-anew-every-turn.md)): at a turn's start, the files the agent last
  knew are compared with what they are now, and those that changed are named - "[Files changed since
  you read them] docs/plan.md, config.yaml" -, given into the transcript where the list changes.
  Claude Code tells its model so (`edited_text_file`).
- **The name alone, or the new content too.** The content helps at once, and costs context -
  issue [542](542-what-is-given-again-unbounded.md) bounds what is given again after a
  compaction for the same reason.
- **What is compared.** Reading every file the agent knew at every turn costs the storage; the
  size and time of a file (`stat`) tell a change cheaply, its digest only where they differ.
- **What holds after being told.** A file named as changed stays refused for a write until the
  agent read it again: being told is not reading.

## Done when

- At the start of a turn, the agent is told which of the files it last knew have changed since.

## History

- **2026-11-06** — Opened from the plan of issue 538, which left it to an issue of its own.
