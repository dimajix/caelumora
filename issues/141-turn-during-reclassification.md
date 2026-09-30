# 141 — A turn running while its channel is lowered loses its answer

**Status:** done
**Area:** Sessions and channels

## The problem

A turn keeps the classification its channel had when it began
(`TurnState::classification`), so that what it reads, publishes and
remembers is judged alike for the whole turn. When the channel is lowered
while the turn runs ([133](133-classification-model.md), step 4a), the
agent goes on classifying its messages as the channel was - above the
channel now. The channel's gate (`ChannelLoop::admits`) refuses them: the
events are dropped with a warning, and the turn's answer never reaches the
channel. Raising the channel during a turn does no harm: the turn's
messages are classified below the channel and admitted.

## Why it matters

- Lowering a channel is a deliberate act of its owner, likely while the
  conversation goes on; losing an answer to it silently is surprising.
- The agent's own transcript keeps the answer, so the agent and the
  channel disagree about what was said.

## Considerations

- **Wait for idle agents.** The reclassification waits until no turn runs
  in the channel, or is refused while one does. Simple and consistent, but
  a long turn - waiting for an approval - blocks the owner.
- **Cancel running turns.** Lowering the channel cancels them, as a
  deliberate cancel would. The answer is lost too, but visibly, and the
  agents answer anew under the new classification.
- **Fit at the gate.** The channel fits messages classified as its previous
  classification into the new one, as it fitted the history. That makes the
  gate change what it admits, and needs the channel to remember what it was.
- **Re-read per message.** The turn takes the channel's classification for
  each message instead of once. That gives up the turn's consistency, which
  the reading rules rely on.

## Done when

- A reclassification during a running turn loses nothing silently, and how
  it is handled is documented in `docs/CONFIDENTIALITY.md`.

## History

- **2026-09-30** — Found while implementing step 4a of
  [133](133-classification-model.md) and noted there; opened at the
  maintainer's request.
- **2026-09-30** — Discussed. Fitting at the gate and re-reading per
  message are ruled out: both would publish, below the channel's former
  classification, what the turn read under it - including content of
  other channels the owner never saw. Leaning towards cancelling running
  turns on a lowering. Such a turn must not be taken over as `/cancel`
  does - its messages are classified above the channel now - but given up
  as a dead branch of the agent's transcript, which depends on
  [145](145-ended-turns-leave-a-record.md).
- **2026-09-30** — [145](145-ended-turns-leave-a-record.md) is done:
  `Turn::give_up_turn` keeps a turn as a dead branch and tells the agent,
  by name only, which tools ran once the turn no longer flows into the
  channel. Cancelling on a lowering needs a reason of its own for it.
- **2026-09-30** — Decided and done. A new control method that only
  interrupts (`recheck_turn`) was weighed and not taken: the existing
  `cancel` suffices, since the channel cancels only when it is lowered,
  when practically every running turn is affected. `ChannelLoop::reclassify`
  cancels the channel's turns and keeps it quiet once the new
  classification is stored, if what the channel held may not be put into
  it as it is now (`ActionType::Publish`, in the reclassifier's name);
  `cancel_actors` is shared with `ChannelLoop::cancel`. Every agent asks
  before each step of a turn whether the turn still flows into the
  channel, and gives it up as `GivenUpReason::Lowered { channel }` if not -
  also after a cancel, instead of taking it over, and also a turn begun
  just before the change or resumed after a restart - telling the users
  with `TurnFailed`. Waiting for an approval now waits on the turn's
  cancellation token too, so `cancel` and `stop` end it at once (before,
  both waited for a decision, and a stopped agent was aborted with its
  turn left in its state). The web UI's release warning for a channel
  says that answers being written are discarded. Refusing a lowering while
  a turn runs is not needed any more.
