# 284 — Automatic context compaction

**Status:** open
**Area:** Context & knowledge

## The problem

Issue [16](16-context-compression.md) gives an agent's context a compaction - a new branch of its
transcript that begins with a summary in place of the oldest messages - and a command to ask for
it, `/compact @agent`. Nothing compacts a context on its own. A session still ends when the model's
context window is full - silently, since Ollama cuts a prompt that is too long without saying so
(issue [209](209-token-usage.md)) - unless somebody watches the gauge and types the command in
time.

The gap is widest where it matters most. Between two turns a person can intervene; within a turn
nobody can, and a turn that works on files makes up to `MAX_MODEL_CALLS` model calls, each adding a
tool result - and tool results are the bulk of a context. The turn that needs the compaction is
the one that cannot ask for it.

## Why it matters

- "A session can run past the model's context window without failing" was 16's first *Done when*,
  and a command alone does not reach it. It is reached here.
- The session that lasts for days - the target of [PLAN.md](PLAN.md) - is this.

## Considerations

- **One trigger, before every model call.** `AgentActor::handle_llm_turn` asks before it resolves
  the turn's context whether the context has to be compacted first. That covers the gap between
  turns as well - the first call of the next turn asks the same question, with what was perceived
  since in the transcript - so there is no second trigger in `handle_waiting`.
- **What is known: the last call.** Nothing counts tokens ahead of a call. What is known is the
  size of the context in the agent's last call (`last_call_tokens`, `AgentChannelState::usage`)
  against its size (`Message::context_size`, issue 209). A share of the size - a parameter
  `threshold`, say `0.8` - leaves room for what was added since the last call, which is not
  counted. An agent whose model reports no context size cannot be compacted automatically - the
  rule has nothing to measure - and says so once.
- **The mechanism is 16's.** `AgentChannelState::compact` takes a cut and the entries that replace
  what is before it; the strategy - the cut, the summary, the skills loaded again - is the agent's
  `CompactionStrategy`'s, the same for a compaction asked for and one made on its own. Within a turn
  the turn's own messages so far are kept verbatim (`Message::turn_id`), so that
  `Transcript::of_turn` finds them when the turn is taken over; the turn's context moves to the new
  head, as it does for `/compact`. The prompt parts the turn placed (`PromptRef::after`) name
  entries of the old branch and have to be placed again on the new one - the `ContextProvider` is
  asked again, as at the turn's start.
- **Where it lives.** The strategy is already a facet of its own, `CompactionStrategy`
  (`crate::compaction`), of which an agent has exactly one - by default `summary_compaction` - and
  which `/compact` asks too. The actor asks it as well; it does not know what a summary is. Whether
  *when* to compact - `needs_compaction`, the threshold - becomes a method of the strategy (a
  strategy that drops old tool results frees otherwise than one that summarises) or stays the
  trigger's is to decide here. Considered and not chosen: the agent sending itself a `/compact`. It
  would reuse everything, but couples the trigger to a command's name and an issuer that does not
  exist, cannot run within a turn, and would record a command nobody gave.
- **The channel is told.** `/compact` answers channel-wide; a compaction made on its own has no
  answer. An event of the agent - `ContextCompacted { turn_id, .. }` - that the channel records in
  its history as it records a turn's end, so that the chat shows it in its place after a reload
  and the gauge (issue 209) can show that the context shrank before the next call reports its
  size. The agent's own history has `HistoryEntry::Compacted` from 16.
- **The summary's call is a call of the turn.** Whether it counts against `MAX_MODEL_CALLS` is to
  decide: not counting it lets a turn that compacts twice go on; counting it is simpler and bounds
  the cost. A turn given up or stopped while the summary is being made loses nothing: the chain is
  changed only after the model answered, as for `/compact`.
- **Old tool results first?** 16 noted that tool results are the bulk of a context and the least
  useful to keep verbatim. A cheaper level before summarising - an old tool result replaced by a
  line that says what it was and that it was dropped - would compact without a model call. Whether
  that is a strategy of its own - a second component providing `CompactionStrategy` - or comes
  first and the summary second is to decide here, with measurements.
- **External agents** keep their context on the remote side; nothing here measures or compacts
  it.

## Done when

- A turn's model calls run past the model's context window: the agent compacts before a call that
  would not fit, within a turn and between turns alike, without anybody asking.
- When it compacts is configured per agent, on its `CompactionStrategy`.
- The channel is told of a compaction and shows it where it happened.

## History

- **2026-10-10** — Issue created: split from [16](16-context-compression.md), whose mechanism and
  `/compact` come first. 16 keeps the command and the branch; the trigger, the threshold and
  telling the channel are here.
- **2026-10-10** — Preparation, without a change of behaviour: how a context is compacted is taken
  out of `/compact` into the facet `CompactionStrategy` (`crate::compaction`), of which an agent has
  exactly one; the default, `summary_compaction`, holds 16's summary and `keep_last`. The command
  component `compaction` only asks it and tells the channel. The trigger here asks the same facet.
