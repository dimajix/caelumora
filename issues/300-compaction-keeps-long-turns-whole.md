# 300 — A compaction keeps a long turn whole and summarises almost nothing

**Status:** done
**Area:** Context and knowledge
**Summary:** A compaction keeps the turn its newest messages reach into whole, so that a long plan is never summarised and almost nothing is freed.

## The problem

`summary_compaction` (issue [16](16-context-compression.md)) keeps at least `keep_last` messages
(default 10) and summarises the rest. So that no turn is cut apart, `cut_at` moves the cut back
to the first message of the turn the oldest kept message belongs to. A plan is one turn, and
usually the largest part of a context: when the last `keep_last` messages reach into it, the whole
turn is kept and nothing of it is summarised.

Found by the maintainer with `/compact @jonathan`. Jonathan's transcript held 55 entries: the
request, one plan turn of 43 messages, two short turns after it. The tenth newest message lay in the
plan turn, the cut moved back to its start, and one message was summarised - the request. The
summary said "Done: None. Open: The entire implementation", and the next answer, given the new
branch, saw nearly everything verbatim, so that it looked as if the compaction had been ignored.

## Why it matters

- A compaction that frees nothing where most of the context is - in a plan - does not do what it
  is for, and says so only by a count in its answer ("1 messages replaced").
- Automatic compaction ([284](284-automatic-context-compaction.md)) keeps the turn under way
  verbatim by its plan, and the turn under way is the long one that does not fit; it runs into the
  same wall.

## Considerations

- **What has to stay together.** A tool's result must not be kept without the call it answers -
  the model would be given a result for a call it never made. That is the only pair a cut must not
  separate. A finished turn cut apart - the rest of it kept, its start summarised, then its
  `TurnFinished` - harms nothing found so far: `Transcript::of_turn`, which collects a turn's
  messages by its end, is used for the turn under way alone. To check: answering again after or
  instead of a message of a turn so cut (`AgentChannelState::retried`, `answers_again_after`).
- **The turn under way** is not cut apart: its messages are collected when it is taken over. For
  `/compact` it is the command's own turn and holds nothing; for 284 it is what has to be
  decided there.
- **Options.**
  - *A - cut within a finished turn too*, moving the cut back only as far as needed to keep a tool
    result with its call: to the assistant message that holds the calls. `keep_last` stays a lower
    bound, met closely. In Jonathan's case: the request and most of the plan summarised, the end of
    the plan and the turns after it kept.
  - *B - move the cut forward to the end of the turn*: no turn is cut apart, but where the last turn
    is the long one, everything is summarised, its last answer included; `keep_last` is no bound
    any more.
- **Decided: A** (maintainer, 2026-10-10).
- **For 284**: its plan keeps the turn under way's messages verbatim; with the same rule it would
  free nothing in a long plan. To be noted there.

## Done when

- A compaction keeps about `keep_last` messages even when they reach into a long finished turn, and
  summarises the rest of that turn with everything before it.
- No tool result is kept without its call; the turn under way is not cut apart.
- Answering again still refuses or works as before for a turn cut apart.

## History

- **2026-10-10** — Issue created on the maintainer's report from trying `/compact @jonathan`,
  analysed from Jonathan's stored state: one message summarised, the plan turn kept whole. Decided
  with the maintainer: option A, cutting within a finished turn, never between a tool call and its
  result.
- **2026-10-10** — Done, as decided. `cut_at` moves the cut back only over tool results, to the
  message that called the tools; a turn of `kept_whole` - the turn under way, and every turn given
  up because the channel was lowered (`GivenUpReason::Lowered`) - is still kept from its first
  message. The second was not in the plan: `Turn::context_until` resolves the part summarised as a
  prefix, and a prefix that holds the start of a lowered turn but not its end would give the model
  what the turn read, which may not flow into the channel any more. Answering again is not changed
  by a turn cut apart: after a compaction its end is no longer last, so both `retried` and
  `answers_again_after` refuse as they did for a turn kept whole. Tests: the cut keeps a result with
  its call, cuts a long finished turn apart, and keeps a turn of `kept_whole` whole; which turns
  those are. AGENT-TURN.md, *Compaction*. Nothing stored changes.
