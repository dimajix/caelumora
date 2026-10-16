# 397 — `AgentChannelState` moves the turn's context after the transcript in four places

**Status:** done
**Area:** Code structure

Part of [391](391-history-and-transcript-review-epic.md).

## The problem

Whatever goes into an agent's transcript while a turn is under way moves the turn's context on to
the transcript's new head, and most of it is seen by the turn (`Work::see_message`). That is one
rule, written in four methods of `AgentChannelState` (`caelumora-model/src/agent.rs`): `perceive`,
`said`, `given` and `compact` each set `turn.work.context.transcript = self.transcript.digest()`,
three of them after `see_message`, each with its own `if let Some(turn) = self.turn.as_mut()`.

`compact` itself, about 75 lines, asks `self.turn.as_mut()` four times because `self.transcript`
is borrowed at the same time; the copies of the entries kept are computed in a `match` inside a
closure that belongs to `TranscriptEntry`. And `Turn::compact` (`caelumora-agents/src/turn/mod.rs`)
afterwards finds the end of the replacement again with `entries().nth(replaced - 1)`, a value
`AgentChannelState::compact` computed as `replacement_end` and did not return.

## Why it matters

- The next thing that goes into the transcript during a turn - and
  [389](389-knowledge-read-in-the-transcript.md) was one - copies the three lines a fifth time, or
  forgets one of them and leaves the turn's context behind its transcript.
- `compact` is the one method of the state that is hard to read, and it is the one every strategy
  and every change to compaction goes through.

## Considerations

- **One private method** that appends an entry to the transcript as the turn under way sees it and
  moves the context on - `fn record_in_transcript(&mut self, entry: TranscriptEntry)` or two, one
  for a message seen and one for the move - called by the four.
- **`compact` takes its turn out**: `let mut turn = self.turn.take()?;` at the start and the turn
  put back at the end leaves no `Option` to check in between; the copy of an entry kept, as it
  stands, becomes `TranscriptEntry::copy_standing(&self, standing: Option<Message>) ->
  Option<TranscriptEntry>`.
- **`compact` returns what `Turn::compact` needs**: the end of the replacement beside the heads
  before and after, so that nothing is recomputed from the transcript.
- No change of behaviour or of stored data.

## Done when

- The transcript's head reaches the turn's context by one method, which every append during a turn
  goes through.
- `AgentChannelState::compact` checks for a turn once, and `Turn::compact` recomputes nothing it
  was told.

## History

- **2026-10-16** — Opened from the review of the history and the transcript
  ([391](391-history-and-transcript-review-epic.md)).
- **2026-10-16** — Narrowed with the maintainer to `compact`, and done there.
  `AgentChannelState::compact` binds the turn under way once (`self.turn.as_mut()?`) - the rest of
  the method touches other fields of the state, which the borrow checker allows -, so the four
  checks for a turn, the `turn_id` read ahead and the second lookup in the `debug_assert` are gone;
  `self.turn.take()`, proposed above, was not taken, since every `?` after it would lose the turn
  without a word. The copy of an entry kept is `TranscriptEntry::copy_standing`, with the type whose
  variants it tells apart. No stored data changes.

  Not taken up: one method for moving the turn's context after the transcript's head - the four
  places set the same field but do different things beside it (`perceive` lets the turn see all it
  heard, `said` sees nothing of the turn's own message, `given` sees and records knowledge,
  `compact` sees, records and counts the summary's call), so a shared method could only wrap the one
  assignment, and `said` would have to be rearranged for it; the tests that resolve the context of a
  turn under way would catch a fifth place forgetting it. Nor does `compact` return the end of the
  replacement: `Turn::compact` finds it with one correct line, and a triple of two `Digest`s and an
  `Option<Digest>` is easy to mix up.
