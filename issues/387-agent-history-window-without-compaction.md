# 387 — An agent that never compacts holds its whole history in memory

**Status:** open
**Area:** Persistence

## The problem

An agent's actor holds its history in a channel from a window on (issue
[379](379-history-stored-as-it-stands.md), `AgentChannelState::keep_window`): from its last
compaction on, and at least the last `WINDOW` (200) entries. An agent that never compacted keeps all
of it - the rule follows the transcript, whose one branch a turn reads back to its start
(`History::produced_on_branch`, for the tokens used).

An external agent never compacts: its transcript starts anew with every call instead
([380](380-transcript-window.md)). Its actor therefore holds every entry of its history it ever had,
though it reads none of it back further than the last few - its calls report no tokens, so nothing
measures what a branch produced. An agent run here that is never compacted - a short context that
never fills, or a model with a large one - holds its history whole as well, as long as its one
branch grows.

## Why it matters

- The history of an external agent grows with every message in its channel, without a bound, and
  is held in memory for as long as its actor runs - what 379 set out to remove.
- With 380 the transcript of an external agent is held only from its last call on, so its history
  is now the one part of its state that is not bounded.

## Considerations

- **What reads the history in memory** (AGENT-TURN.md, *What is held in memory*): what a turn said,
  read from where it began; what a branch produced, back to the compaction that began it; the
  messages a request to answer again takes back; the last message, whether the last turn was given
  up. For an external agent only the last few of these apply: it has no turns of its own model, no
  tokens, and no answering again (`RetryRefused::Elsewhere`).
- **A window by kind.** An external agent could keep the last `WINDOW` entries alone, whatever it
  compacted - the window of a channel. An agent run here keeps the rule it has.
- **A window by its branch.** The window could follow the root of the transcript's active branch
  for both kinds, as the transcript's window does: for an external agent that is its last call,
  for an agent run here its last compaction or its start. Then `produced_on_branch` keeps its
  reach, and an agent run here that never compacts still holds all of it - which is bounded by its
  context, since a branch that fills it is compacted.
- **Measured, not assumed.** Before changing the rule, check what of the history an agent run here
  reads without a compaction, and whether an agent whose context never fills grows in practice.

## Done when

- An external agent's actor holds a bounded part of its history.
- AGENT-TURN.md and STORAGE.md say what each kind of agent holds of its history.

## History

- **2026-10-16** — Opened from the work on [380](380-transcript-window.md): with the transcript's
  window, an external agent's history is the part of its state that still grows without a bound.
