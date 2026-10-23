# 284 — Automatic context compaction

**Status:** done
**Area:** Context and knowledge
**Summary:** An agent's context is compacted only when somebody asks for it; nothing compacts it before a model call that would not fit, least of all within the turn that makes it.

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

## Decisions taken

Agreed with the maintainer on 2026-10-14; what was weighed and set aside is under *History*.

- **One compaction, cut by tokens.** `cut_at` no longer counts `keep_last` messages and no longer
  keeps the turn under way whole: where it cuts - before the turn under way or within it - follows
  from the amount of tokens to free alone. There is no second compaction in the same call; one
  that did not free enough is followed by another before the next model call. A turn given up as
  `Lowered` stays whole, as before.
- **The cut uses the positions the calls recorded.** A message of the agent's own model carries
  `prompt_tokens` and `completion_tokens` of its call. Their sum is not the message's size but its
  position in the context - how many tokens stand up to and including it -, and `prompt_tokens`
  alone is the position directly before it. Only the agent's own model messages (its `sender_id`,
  role `Assistant`) made on the active branch since the last compaction count: the copies kept and
  the summary carry the tokens of calls over the old context, another agent's messages those of
  its own model. A position includes what every call is given, the system prompt, so what a cut
  frees is the difference to where the branch begins.
- **Within a turn, the cut lies directly before an own message.** Within a turn there are only
  the agent's own messages, their tool results and what it is given; what others say waits until
  the turn has ended. A cut before an own message keeps every tool result with its call without a
  rule of its own - the backward rule of `cut_at` goes -, and its position is exact. The newest own
  message and its results are never summarised: the model reacts to a fresh result as it stands.
  A compaction happens only before a model call, when every call of the last step has its result,
  so no open approval or question is in the part summarised.
- **Before the turn, where exactly matters little;** the turn's input is kept as it stands. The
  agent's own last message is not protected there: an agent that only listened for many rounds has
  to have it summarised too.
- **What was added since the last position is estimated.** Positions exist only at own messages.
  An agent that only listens makes no calls - perceiving is no turn -, so its last call precedes
  everything heard since, and a trigger that measured the last call alone would see a fraction
  and let the call overflow. The fill is the position of the last own call plus an estimate
  (characters / 4) of the entries after it - within a turn the newest tool results, for an agent
  that listened everything it heard; before any call on a new branch, the whole branch. The same
  estimate places a cut behind the last position. Positions where they exist, the estimate only in
  the gap after them.
- **Whose the summary is follows from the cut.** A summary of messages of the turn under way holds
  what the turn read: it is the turn's (`Message::turn_id`, the turn's label) and is withheld with
  the turn if it is given up as `Lowered`. A summary of what precedes the turn alone is no turn's:
  it has no `turn_id` and is classified by what it summarises - the call writing it is given the
  transcript only, no prompt parts. Today it is the turn's in either case
  (`AgentChannelState::compact`): classified too high, and a turn given up as `Lowered` would
  withhold the whole conversation before it.
- **A turn's answer is read from the history, not the transcript.** The transcript is what the
  model is given, the history what was said, and the history tells the two apart already: what a
  turn says is `Said` with the context it was produced from, what it is given and what replaces
  older messages without. `Turn::commit` collects the answer by `Transcript::of_turn` today, which
  after a cut within the turn lacks what was summarised and holds the summary and what was given
  again - the `TurnFinished` would carry them, and the moderator decide by them who responds.
- **Within a turn, its plan and the current step are named again.** The plan stands only in the
  `create_plan` call and its result, the current step in a prompt part placed after the turn's
  messages (`place_turn_prompt`), which `compact` drops; and that names the step alone.
  `Turn::compact` - which holds the plan and the prompt parts, and through which every compaction
  passes - places it again with an overview: which steps are done, which is under way. The strategy
  knows nothing of plans.
- **Prompt parts are carried over, not dropped.** `compact` drops every part placed after an
  entry today. Besides the step of a plan, that is the part the `ContextProvider` places before the
  turn's input (`BeforeTurnInput`: notes, memory, who is in the channel), which would be gone for
  the rest of the turn wherever the cut lies, and which the summary does not hold. A part whose
  entry was kept follows its copy, one whose entry was summarised follows the replacement. That
  carries the step of a plan over as well, so naming the plan again only adds the overview. The
  `ContextProvider` is not asked again, as considered above: it would not place the step again, and
  it runs once per turn (issue 285).
- **The summary's call does not count against `MAX_MODEL_CALLS`.** The limit bounds the turn's
  work, and a turn that compacts would otherwise reach it sooner the longer it works. Nothing runs
  away by it: a compaction comes only before a model call, which counts, so a turn makes at most as
  many compactions as calls. `summarise` counts the call today (`Turn::count_model_call`);
  that goes, and with it `CompactionError::CallLimitReached`.
- **A compaction that fails ends the turn**, given up as `Failed`, as any error of a turn: the call
  it was to make room for would not fit. The transcript is unchanged, since it changes only after
  the model wrote the summary. An agent whose model reports no context size is not compacted - the
  rule has nothing to measure -, which is no failure: it says so once, and its calls are made.

- **The channel is told by activities, not by events of compaction.** What an agent does of its
  own accord, besides answering, is announced generically - a compaction is the first such
  activity, more may follow -, complementing the commands, which it carries out when asked:
  - `ActorState::Busy`, the counterpart of `CarryingOutCommand`, announced by `StateChanged` as
    every state is. Within an answer it belongs to the turn (`in_turn`); after a restart it goes on
    as `Planning`, and the trigger asks again - the transcript changes only at a compaction's end.
    An activity between turns would need a turn of its own, as a command has; that is settled
    when there is one.
  - `ActivityStarted { turn_id, .. }` and `ActivityFinished { turn_id, .. }`, shaped as
    `CommandStarted` and `CommandExecuted`: what the activity is, and at its end what came of it
    in words of its own, as a command answers - an activity may come from a plugin as well. The
    channel records the end in its history, as it records a command, so that the chat shows it in
    its place after a reload; users see both, agents hear neither. An activity that fails ends
    with the turn, `TurnGivenUp`, as a `CommandFailed` ends a `CommandStarted`.

  `/compact` keeps the events of a command - `CommandStarted`, `CommandExecuted` or
  `CommandFailed`, in the state `CarryingOutCommand` - and sends no activity. Both triggers share
  the compaction itself, the strategy and `Turn::compact`; what is announced around it is the
  trigger's, the actor's for a compaction of its own, `carry_out` for a command. The `Turn` needs
  no means of sending events for it.

Known limits, not solved here:

- What an agent heard since its last call can be more than its model's context window. The call
  writing the summary is then given too much and cut short itself; summarising in stages would
  help.
- A single tool result that fills the context window cannot be compacted away. The terminal tool
  limits what it returns (`max_output_bytes`), other tools - those of MCP servers - do not.
- Nothing is compacted before the first measured call on a branch (below, *Reconsidered*): an
  agent that never called its model, or just compacted, makes one call that may overflow; the
  call after it is measured and compacts.

## Implementation plan

Written 2026-10-14 from the decisions above, with the code read again.

### Reconsidered

The decisions hold. Reading the code again settled what they left open, and sharpened four of
them:

- **A position is known at a message whose recorded context names the active branch.** The
  history records with every message a turn says the context it was produced from
  (`HistoryEntry::Said { context: Some(..) }`, `ContextRef::transcript`); a copy kept through a
  compaction names the old branch, the summary and what is given again are recorded without a
  context. So "an own message made on the active branch since the last compaction" needs no
  marking: its context's transcript digest lies on the active path. The copies' token counts stay
  as they are - they count once for the usage, where first recorded.
- **The trigger fires only once a position is known on the branch.** Right after a compaction
  every position is an estimate of the new branch; an estimate that errs upwards would compact
  again before any call, and once more after that. Requiring one measured call on the branch
  bounds it: at most one compaction per model call, and the call in between measures. Its cost
  is the third known limit above.
- **What is kept is a share, not a count.** `keep_last` counted messages, which says nothing of
  tokens; it becomes `keep`, a share of the context size kept behind the cut (default `0.25`),
  measured exactly as the difference of two positions - what every call is given regardless, the
  system prompt, cancels out. Where no size is known - `/compact` on a model that reports none -
  the share is of the context as it stands. The trigger's `threshold` (default `0.8`) is a share
  of the size too, and without a size there is no trigger.
- **The gauge is fixed on the way.** `TokenUsage::last_call_tokens` is wrong after a compaction
  today: the last call is the summary's, over the old context. `TokenUsage` gains
  `context_tokens`, the tokens the context holds now as the positions and the estimate say, and
  the gauge (web UI, issue 209) reads that; `last_call_tokens` stays what it is.

Settled details:

- The cut candidates: within the turn under way, its own messages but the first - a cut before
  the first would summarise the turn's input while keeping the turn -; before the turn, every
  message that is not a tool result and lies in no turn given up as `Lowered`. The cut is the
  oldest candidate whose distance to the end is at most `keep`; if none is, the newest. Nothing
  to compact if there is no candidate or nothing before it.
- The summary of what precedes the turn alone is classified `Classification::join_all` of the
  messages it summarises, the turn's label where they have no audience in common (as today).
  What is given again is stamped with the turn under way, as `AgentChannelState::given` stamps.
- The plan is named again as one prompt part: the overview of its steps - done, under way, to
  come - followed by the current instruction. Its text is a function of the plan's state, so a
  compaction recognises the part it placed before by the digest of that text and replaces it;
  nothing is added to `PlanState`.
- `Busy` is interruptible: a cancel ends the summary's call as it ends any call of the turn.
  After a restart the turn goes on in `Busy` and compacts; the transcript changed only if the
  compaction had ended, and then the stored state is `Planning`.
- The activity events carry words, as a command's answer does, since an activity may come from a
  plugin one day: `ActivityStarted { turn_id, activity }` - "compacting its context" -, and
  `ActivityFinished { turn_id, activity, output }`. The channel records the end with its output,
  which is everybody's. What a compaction did is worded once, `Compaction::describe`, for the
  activity and for `/compact` alike.

### Changed types and interfaces

`caelumora-model`:

- `Message::estimated_tokens()` - content, tool calls and results, characters / 4.
- `AgentChannelState::token_positions()` - for every entry of the active path the tokens the
  context holds before it: exact at an own message whose recorded context names the active path
  (`prompt_tokens`; after it `+ completion_tokens`), estimated beyond the last exact one; and
  `context_fill()` - the tokens after the last entry, and whether a position was measured on the
  branch. `usage()` fills `TokenUsage::context_tokens` from it.
- `AgentChannelState::compact` - takes the replacement messages as they are (`turn_id` set by the
  caller), carries the turn's prompt parts over: a part after a kept entry follows the entry's
  copy (a map of old to new digests, made as the copies are pushed), one after a summarised entry
  follows the last entry of the replacement.
- `MessageHistory::said_in_turn(turn_id)` - the messages a turn said, as they stand: `Said`
  entries with a context. `Transcript::of_turn` goes.
- `HistoryEntry::ActivityFinished { turn_id, actor, activity, output, at }`, with its digest
  (`digest.rs`).
- `ActorEvent::ActivityStarted`, `ActorEvent::ActivityFinished` (above).
- `ActorState::Busy` - appended to `ALL`, in a turn, interruptible.

`caelumora-runtime`:

- `CompactionStrategy::is_due(&self, tokens, size) -> bool` - the threshold is the strategy's.
- `summary_compaction`: `SummaryCompactionConfig { threshold, keep }` replaces `keep_last`;
  `cut_at` cuts by positions (above); `kept_whole` keeps the `Lowered` turns alone; `summarise`
  no longer counts the call, `CompactionError::CallLimitReached` goes; the summary's `turn_id`
  and classification by the cut; `Compaction::describe(agent_name)`.
- `Turn::compact` - after `AgentChannelState::compact`, places the plan's overview part if a plan
  is in progress (`plan.rs`: `plan_overview_instruction`); `Turn::messages` reads the history.
- `Turn::commit` - the answer from `MessageHistory::said_in_turn`.
- `AgentActor` - holds `compaction: ComponentImpl<dyn CompactionStrategy>` as it holds the
  `context_provider`; `handle_llm_turn` asks `context_fill` and `is_due` before it counts the call
  and returns `Busy`; `handle_busy` announces `ActivityStarted`, compacts under the cancel token,
  announces `ActivityFinished`, returns `Planning`; a failure is the turn's (`fail_turn`).
- `compaction::command` - the answer from `Compaction::describe`.
- `PolicyModerator::route` - the activity events to the users, as `TurnGivenUp`.
- `ChannelLoop::append_history` - `ActivityFinished` recorded as `CommandExecuted` is.

`caelumora-web-ui`: `ActorState` and the working label for `Busy` - "Working: " and the last
`ActivityStarted`'s words -; the two events as notices in the chat, the started one replaced by
the finished one (as a command's are, `commandNotice`); `HistoryItem` `activity_finished` shown in
its place after a reload (`ChatView`, `HistoryPanel`); the gauge from `context_tokens`.
`caelumora-tui`: the two events printed as the command events are.

### Steps

Each one reviewable on its own; the first three change no behaviour of `/compact` but what is
named.

1. **The answer from the history.** `MessageHistory::said_in_turn`, `Turn::commit` and
   `Turn::messages` use it, `Transcript::of_turn` goes. Same messages as before for every turn
   compacted so far, since none was cut apart.
2. **Prompt parts carried over, the plan named again.** `AgentChannelState::compact` carries the
   parts; `Turn::compact` places the overview. `/compact` runs in a command turn without parts of
   its own: unchanged.
3. **The summary's owner by the cut.** `AgentChannelState::compact` stops stamping; the strategy
   stamps and classifies. `/compact` always cuts before its command turn: its summary is no
   turn's from here on, classified by what it summarises - the fix of the classification.
4. **Measuring.** `Message::estimated_tokens`, `token_positions`, `context_fill`,
   `TokenUsage::context_tokens`; the gauge reads it. Fixes the gauge after `/compact`.
5. **The cut by tokens.** `cut_at` by positions, `threshold` and `keep` replace `keep_last`,
   `kept_whole` reduced, the summary's call uncounted. `/compact` keeps by tokens from here on.
6. **The trigger and the activity.** `is_due`, `Busy`, `handle_busy`, the actor's strategy, the
   events, moderator, channel history, digest. The REST stream carries the events as it carries
   every `ActorEvent`.
7. **The frontends.** Web UI and TUI as above.
8. **Documents and the issue.** Below.

### Tests, documents, stored data

- Tests: `message_history` (`said_in_turn`), `agent.rs` (parts carried over, the owner left to
  the caller, positions and the fill with copies and a summary on the branch), `turn/ending.rs`
  (the answer of a turn cut apart holds what was summarised and not the summary), `turn/plan.rs`
  (the overview placed and replaced), `summary_compaction` (`cut_at` by positions: within a turn
  before an own message but the first, before it any message, `Lowered` whole, the newest when
  nothing fits; the owner and classification by the cut), `actor/agent` (a turn that is due
  compacts before its call, once per call, not without a measured position; a failed compaction
  gives the turn up; `/compact` sends no activity), `policy_moderator` (routing), `channel/run`
  (the history entry), `digest`, the TUI's and web UI's own. No action in anybody's name changes:
  nothing for the security suite.
- Documents: `AGENT-TURN.md` (*Compaction*: when, the cut, whose the summary is, the parts
  carried, the answer from the history; *The cognitive cycle*: `Busy`; *Tokens used*:
  `context_tokens`), `COMPONENTS.md` (`summary_compaction`'s parameters, `CompactionStrategy`),
  `COMMANDS.md` (`/compact` keeps by tokens), `CHANNELS.md` and `REST-SERVER.md` (the activity
  events, the history item), `WEB-UI.md` (the notices, the gauge), `LLM-MODELS.md` if a model
  wants another `threshold`.
- Stored data: none to delete. `ActorState::Busy` is appended, `ActivityFinished` is a new kind
  of entry; `TokenUsage` is computed. A configuration naming `keep_last` fails to load and is
  changed to `threshold` and `keep`.

### Noticed beside it

- `AgentChannelState::usage` reads `context_size` from the last message that reported tokens,
  whichever branch it came from; a model changed between turns would be misread until its next
  call. Harmless, noted.
- The `given_up_notice` the resolver adds for a turn given up is not in any position or estimate;
  a few dozen tokens, ignored.

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
- **2026-10-10** — From [300](300-compaction-keeps-long-turns-whole.md): `summary_compaction` cuts a
  finished turn apart now, keeping only a tool's result with its call, but still keeps the turn
  under way whole (`kept_whole`). The plan here - the turn under way's messages kept verbatim -
  frees nothing where it is most needed: a long plan is one turn, and the turn that does not fit.
  Cutting the turn under way apart has to settle what taking it over then collects of it
  (`Transcript::of_turn`); that is to decide here.
- **2026-10-14** — Discussed with the maintainer and decided (see *Decisions taken*). Weighed and
  set aside: keeping the turn under way whole - it frees nothing where it is needed most;
  compacting twice in one go, first before the turn and then within it - the next call compacts
  again if one did not free enough; a cut directly after an own message's results - within a turn
  the same place as before the next own message, between turns at a position nobody knows;
  estimating the size of every message by its characters - the positions the calls recorded are
  exact, the estimate fills only the gap behind the last of them; never summarising the agent's own
  last message, between turns as well - an agent that only listens would then never be compacted;
  asking the `ContextProvider` again after a compaction - it would not place the step of a plan
  again, and runs once per turn; counting the summary's call against `MAX_MODEL_CALLS` - a turn
  that compacts would reach the limit sooner the longer it works; making the call anyway after a
  compaction failed - it would not fit; events of compaction of their own (`ContextCompacted`) -
  the generic activity covers it and the occasions to come; a message carried by `StateChanged` -
  a state is announced at every change and recorded nowhere, so it could not be shown in its place
  after a reload; announcing from the code both triggers share, which would have needed the
  `Turn` to send events - each trigger announces in its own way.
- **2026-10-14** — Implemented as planned, in the steps of the plan: the answer of a turn from the
  history (`MessageHistory::said_in_turn`, `Transcript::of_turn` gone); prompt parts carried over
  and the plan named again (`Turn::compact`, `name_plan_again`); the summary's owner and
  classification by the cut; the context measured (`AgentChannelState::token_positions`,
  `context_fill`, `TokenUsage::context_tokens`, `Message::estimated_tokens`); `summary_compaction`
  cutting by positions with `threshold` and `keep`; the trigger in `AgentActor::handle_llm_turn`,
  the state `Busy` and `handle_busy`, `ActorEvent::ActivityStarted`/`ActivityFinished`,
  `HistoryEntry::ActivityFinished`; web UI and TUI. Beyond the plan: a turn tries to compact once
  before each call, kept with it (`TurnState::compaction_tried_at`), so that a strategy finding
  nothing to compact does not ask again before the same call; a call's position is told by its
  message's id, which a backend gives each message anew. Noticed beside it: the channel admits
  `ActivityFinished`, as it admits `PlanStepStarted` and `CommandExecuted`, from any sender without
  asking whether it is an agent of a turn - what it records in the history rests on that.
- **2026-10-14** — Reviewed with the maintainer, and changed in three points:
  - *When* is no longer the strategy's. `CompactionStrategy` answers how a context is compacted;
    whether it is due before a call is a facet of its own, `CompactionPolicy`, of which an agent has
    exactly one - by default `threshold_compaction_policy`, with `threshold`. `summary_compaction`
    keeps `keep` alone. So the *Done when* "configured on its `CompactionStrategy`" holds for how,
    and when is configured on its policy.
  - The measuring moved into the `ContextResolver` (`ContextResolver::token_positions`, offered as
    `AgentWork::token_positions`), and out of `AgentChannelState`. It estimates what the model is
    given as the resolver gives it - a message's reasoning only where its rule gives it back, the
    sender's marker, a `Lowered` turn reduced, the prompt parts where they are placed - instead of
    the stored messages; and a measured position is only the one before a message, its call's
    `prompt_tokens`, since `completion_tokens` count reasoning the next call may not be given.
  - `TokenUsage::last_call_tokens` is gone: nothing read it any longer, `context_tokens` says how
    full the context is, and what a single call used stands at its message.
- **2026-10-14** — What the channel records of a turn besides its messages - `PlanStepStarted`, a
  `CommandExecuted` of a turn, `ActivityFinished`, `TurnFinished`, `TurnGivenUp` - and
  `ActivityStarted` are admitted from an agent in its own name alone (`ChannelLoop::admission`);
  before, a user's actor could have had the history record a turn's step or end. Done here, not in
  an issue of its own, as the maintainer asked.
