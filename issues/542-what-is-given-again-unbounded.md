# 542 — What a compaction gives again is bounded per file, not in all

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** After a compaction, every file the agent read whole in the part replaced and still needs is given again, each bounded by `max_output_bytes` but not all of them together, so a context can stay above the threshold and be compacted again and again, giving the same files each time.

## The problem

Issue [538](538-workspace-knowledge-without-a-place.md) made `workspace` a `KnowledgeRestorer`:
after a compaction, the files the agent read or wrote whole in the part replaced are given again
as they are now, each no larger than a result holds (`max_output_bytes`, 64 KiB unless
configured). Which of them the work still needs is the strategy's to answer (issue
[413](413-compaction-restores-unused-skills.md)); `summary_compaction` asks its model, and gives
all of it again where the answer cannot be read, or none comes.

Nothing bounds what is given again in all. An agent that read twenty files in a long piece of work
is given up to twenty times 64 KiB again - some 300,000 tokens - right after the context was made
shorter.

## Why it matters

- **A compaction that does not shrink the context.** What is given again can put the context
  above the threshold at once; the next compaction finds the same files in the part it replaces
  and gives them again, and the context never gets smaller - each time at the cost of a call of
  the model.
- **With skills it hardly mattered**: they are short and few. Files are neither.

## Considerations

- **A budget of the restorer.** A parameter of `workspace` - `max_restored_bytes`, say - bounds
  the files given again together; what does not fit counts as lost. The component knows its files
  and can choose which come first: those read last, which belong to the work still open more
  likely than those read first.
- **A budget of the compaction**, tied to `keep`, shared by every restorer: more general, but the
  compaction would have to weigh kinds against each other, and knows no kind of knowledge by
  design (issue 323).
- **Telling the strategy the sizes**, so that its model chooses within a budget.
- Claude Code gives files again after a compaction within a number of files and a budget of
  tokens.
- **Leave it**: the model's answer is mostly readable, and the strategy chooses.

## Done when

- What a compaction gives again leaves the context below its threshold, or is bounded so that it
  cannot keep it above it.

## History

- **2026-11-06** — Opened from the review of issue 538, step 4.
