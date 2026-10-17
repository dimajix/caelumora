# 390 — How full an agent's context is between turns is estimated with a profile from its history

**Status:** done
**Area:** Context and knowledge
**Summary:** How full an agent's context is between turns is estimated with the model profile of a message from its history, not with the rules of the model it is run with now.

## The problem

What an agent's model calls used is read through `AgentChannelState::usage` (issue 209): the sums
of all calls, kept as they are recorded (`used`, `TokenUsage::counted`), and how full the context
is now (`context_tokens`). The last is not the last call's measure but read anew from what the
model is given next (`next_context_tokens`, issue 284): the last call measured the context before
it, and since then a compaction may have replaced the branch - the summary's call measured the old
one -, a tool may have returned a large result, messages may have been heard, changed or taken
back.

Within a turn it is resolved against the turn's context, with the profile of the turn's model.
Between turns there is no context to resolve against, and `next_context_tokens` takes the
`ModelProfile` from the history: that of the last message the agent said with a context
(`HistoryEntry::Said`), `ModelProfile::default()` if there is none.

## Why it matters

- **The history is informational** ([387](387-agent-history-window-without-compaction.md)): it is
  the record of what was said, not where the rules of the agent's model are found. The profile it
  holds is that of a model the agent was run with, not of the one it is run with now.
- **Since 387 the history is held in part**: an agent that said nothing within the last `WINDOW`
  entries has no message to take the profile from, and is estimated with the default.
- What the profile changes is small - it is applied only to what was added since the last call
  measured the context: the reasoning a model with `ReasoningHistory::Always` is given back, the
  marker before the messages of others (`SenderFormat`) - but it is the wrong source.

## Considerations

**Who reads it.** Between turns only the member's details read it (`MemberView::snapshot`, for the
web UI's gauge and statistics), from the agent's state as stored, whether its channel runs or not.
The compaction reads `usage` only within a turn, and only its `context_size`: how full the context
is, it resolves itself (`token_positions`).

**Who knows the current model.** The agent's actor: `AgentActor` holds its `LLMClient`, whose
`profile()` it gives every turn (`begin_turn`) and its command turns (`AgentBody::profile`); an
external agent's is its remote's (`ExternalAgent::profile`). The view knows none of it, and a
channel that does not run has no actor.

Options, none decided:

- **A - The state keeps the profile of its model.** `AgentChannelState` gets a `profile`, which the
  actor sets as it starts in the channel and every turn begins with; `next_context_tokens` reads
  it. Works for a channel that does not run. A field stored, set in two places, the state stored at
  a start where the profile changed; and it is the profile the actor last ran with, not the one it
  would run with now.
- **B - The profile of the last turn.** `begin_turn` keeps its profile in the state. Simpler than
  A, but a model configured anew shows only after the next turn.
- **C - The caller passes the profile** (`usage(agent, profile)`). The view does not know it.
- **D - The running actor answers its profile.** `Actor::model_profile()`, as `tool_sets()`,
  `components()` and `commands()` answer what the actor has now; the view passes it to `usage`.
  Nothing stored; nothing for a channel that does not run, which falls back to a default.
- **E - The running actor answers how full its context is.** `Actor::usage()`: the one who holds
  the state in memory and knows its model computes it; the view asks it, and reads the stored
  `used` when the channel does not run. The view does not compute at all - but what a channel that
  does not run shows is the last call's measure, wrong after a compaction until the next call.
- **F - The actor stores how full its context is.** Whenever it stores its state, the actor
  computes `context_tokens` with its own model's profile and stores it with `used`; the view reads
  what is stored and computes nothing, whether the channel runs or not. Computed at every store,
  over the active branch, which the actor holds anyway.
- **G - No profile.** Estimate between turns with `ModelProfile::default()`: the difference is
  small and only shown. Removes the read of the history, but keeps an estimate made with rules that
  are not the model's.
- **H - No estimate between turns.** Show the last call's measure. After a compaction the gauge
  shows the old, full context until the next call - what 284 fixed.

## Done when

- How full an agent's context is between turns is estimated with the rules of its current model, or
  by whoever knows them, never with a profile read from the history.
- AGENT-TURN.md (*Tokens used*) says where it comes from.

## Decision

Decided on 2026-10-16 (see History): none of A to H. Every one of them resolves the context anew,
through `token_positions` and the path it is given, each time the usage is asked for - and the
member's details ask for it again and again. Too expensive for a number that is only shown.

**I - The last state, kept.** How full the context is, is the last call's prompt and answer, as
`TokenUsage::counted` keeps it with every message recorded. The one time that is wrong by much is a
compaction: the summary's call measured the old branch. So `AgentChannelState::compact` estimates
the new branch once, when it is made - with the turn's context, so with the profile of the model the
turn runs with - and keeps the estimate in `used`. Nothing is read anew between turns, nothing is
read from the history, and `usage` and `next_context_tokens` go: `used` is what is read.

What was added since the last call - a tool's result, what was heard - is not in it until the next
call measures it. That is accepted: it is a gauge, and the compaction reads how full the context is
before a call for itself (`token_positions`), with the turn's model.

## History

- **2026-10-16** — Opened from the discussion of
  [387](387-agent-history-window-without-compaction.md) and
  [389](389-knowledge-read-in-the-transcript.md): `next_context_tokens` is the last reader of the
  history that concerns what the model is given. The maintainer wants the current model there;
  none of the options above is decided yet.
- **2026-10-16** — Decided and done as *I*: `compact` estimates the new branch once and keeps it in
  `used.context_tokens`; `AgentChannelState::usage` and `next_context_tokens` are gone, and the
  view, the compaction policy and the strategy read `used`. Between turns the gauge shows the last
  call's measure, its answer included. AGENT-TURN.md (*Tokens used*) and WEB-UI.md say so. Stored
  data stays readable.
