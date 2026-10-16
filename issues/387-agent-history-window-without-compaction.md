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

## Decision

Decided on 2026-10-16 (see History). The window is not the cause; the cause is that the model's
input is built in part from the history. **An agent's history is informational: it is the record
of what was said and what it was produced from, and it is never read to build what the model is
given. That is built from the transcript alone.** Once nothing that builds the model's input reads
the history, its readers in memory need the turn under way and a short end of it, for both kinds of
agent alike, and the history's window no longer follows the transcript's.

Two places read the history to build the model's input today:

1. **How full the context is** (`ContextResolver::token_positions`), which decides when a
   compaction is due and where it cuts. A message's `prompt_tokens` counts what stands before it
   only where it was said; a compaction copies the messages it keeps into the new branch with the
   counts of their originals, and its summary carries the count of the call over the old branch.
   The transcript does not say where on a branch the compaction's part ends, so
   `History::produced_on_branch` finds the `HistoryEntry::Compacted` that began the branch and the
   context each message was recorded with. Here.
2. **What a compaction gives again** (`given_read` in `SummaryCompaction`), read from the knowledge
   the history recorded with each message. Its own issue:
   [389](389-knowledge-read-in-the-transcript.md).

`catch_up` reads the history for the last message the agent heard, which decides from where it
takes in what it missed - into its transcript. It is the position up to which an actor has heard
its channel, which [223](223-full-inbox-drops-events.md) needs as well; it stays as it is here and
is noted there.

### Plan

1. **The rule in the documents.** AGENT-TURN.md and the doc comment of `History` say that the
   history is informational and the model's input built from the transcript alone.
2. **`TranscriptEntry::Compacted { at }`**, as `HistoryEntry::Compacted` is for the history and
   the turn's ends are for both (issue 258). `AgentChannelState::compact` writes it after the
   replacement and the entries kept: everything before it on the branch came from the compaction,
   everything after it was added where it stands. `to` is its digest, and the turn's context moves
   to it. It gives the model nothing. `from` and `turn_id` stay in the history's entry alone.
   - `token_positions` takes the measure of a message of the agent's only after the last
     `Compacted` on the path, from the root if there is none - and reads the transcript alone.
     `History::produced_on_branch` goes, and with it the `ContextResolver`'s history: what builds
     the model's input cannot reach it.
   - `retried` refuses an answer followed by a `Compacted` (`RetryRefused::NotLast`): it may
     already be in a summary.
   - A `Compacted` in the part a later compaction keeps is not copied, as a change of a message
     is not.
   - An external agent's `start_anew` writes none: it copies nothing, and the root is the border.
3. **The history's window.** `AgentChannelState::keep_window` keeps the history from where the
   turn under way began (`said_in_turn`, for the turn's answer) and at least the last `WINDOW`
   entries, whatever the agent compacted. The readers left are informational: the turn's answer
   for the moderator, whether a message taken back was said, the anchor of notes and memory, the
   profile `next_context_tokens` estimates with between turns, and `catch_up` (above).
   `Turn::messages`, used only in tests, goes.
4. **Documents.** AGENT-TURN.md (*What is held in memory*) and STORAGE.md say what each kind of
   agent holds of its history, and where the transcript marks a compaction.

Stored data: the agents' states and transcripts are to be deleted - a transcript compacted before
holds no `Compacted`, and its copies would count as measured.

## History

- **2026-10-16** — Opened from the work on [380](380-transcript-window.md): with the transcript's
  window, an external agent's history is the part of its state that still grows without a bound.
- **2026-10-16** — Discussed: the history of an agent is informational and never used to build the
  model's input. The fix is to take the history out of what builds it, not to choose another
  window; see *Decision*. What a compaction gives again was split off as
  [389](389-knowledge-read-in-the-transcript.md).
