# 258 — What a turn said is lost from the history when it is internal or the turn is given up

**Status:** open
**Area:** Sessions and channels

## The problem

A message an agent's turn publishes reaches the channel's history only if the turn ends
normally and the message is not `internal`. Two kinds of message are shown live and then lost:

- **The steps of a plan.** While an agent executes a plan (`create_plan`), every message of a step
  is `internal` (`Turn::in_plan_step`): the step's answer, its tool calls and their results. Such
  messages are published as `IntermediatePartialGenerated` and `IntermediateMessageGenerated`, kept
  in the agent's transcript, and left out of everything else: `Turn::commit` passes only the
  messages that are not internal on to `FullResponseGenerated` (`agent/turn/ending.rs`), the
  channel fills its history from that event alone (`ChannelLoop::append_history`), and the agent's
  own history leaves them out as well (`AgentChannelState::record_turn`).
- **The messages of a turn given up** - failed, stopped, asked for again (issue
  [235](235-retry-of-an-answer-under-way.md)), or the channel lowered below it (issue
  [141](141-turn-during-reclassification.md)). `Turn::give_up` keeps them on a branch of the
  agent's transcript that is no longer active (issue [145](145-ended-turns-leave-a-record.md)),
  tells the agent only which tools ran (`told_of_executed`), and the channel's history never gets
  them. The users saw them as they were said; after a reload they are gone, and `TurnFailed`, which
  said why, with them.

The web UI makes the first case worse: it has no branch for the two intermediate events
(`ChatView.tsx`, `handleMessage`), so from the moment a plan is approved until the answer after its
last step, nothing of the agent appears - no streamed text, no tool call, no result, and the
workspace's file tree is not refreshed. The TUI shows them as `[working]`.

So a tool call an agent makes without a plan is shown and kept in the history; the same call as a
step of a plan, or in a turn that later fails, is not. The flag `internal` was introduced with the
refactoring of the planning phase (`13756dd`) without a recorded reason; the comment at
`Message::internal` calls the history "the user-facing/audit trail", which is what these messages
are missing from.

## Why it matters

- What an agent did - which commands it ran, which files it changed - must be traceable. Its tool
  calls have effects whether or not its turn ends well, and whether or not it worked through a
  plan.
- When what is kept depends on how a turn ended, nobody can tell beforehand what will be there
  afterwards.
- The agent itself is told of a given-up turn's calls only by name and arguments in a runtime
  message, not by the calls and results it made; and to the user, a long plan looks as if the agent
  had stopped answering.

## Considerations

- **Keep only the steps of a plan, through `FullResponseGenerated`.** Decided first: the event
  carries every message of the turn, internal ones included, and the channel keeps storing on it
  alone - so that a turn given up still reaches nothing. Superseded by the principle below: it
  would keep the plan's steps, but not the calls of a turn that failed, and it makes what is kept
  depend on how the turn ended.
- **Keep given-up turns only for some reasons** - failed and stopped, not asked for again, not
  lowered. Set aside: every exception is a rule to learn, and the effects of tool calls do not
  depend on the reason.
- **Make the agent's transcript readable over REST** and show it per turn under the answer. Set
  aside: the transcript is the agent's own view, including what it heard, so it needs a permission
  question of its own; and the history would still differ between one kind of turn and another.
- **Nothing said is unsaid (the decision).** Every complete message a turn publishes stays - in the
  channel's history and in the agent's transcript and history, on the active path - however the
  turn ends. A message still being streamed when the turn ends was never complete and is not kept.
  A turn given up leaves a record of that, with the reason. In detail:
  - **The channel stores each message as it arrives:** every `CompleteMessageGenerated` and
    `IntermediateMessageGenerated` it admits goes into its history. `FullResponseGenerated` is no
    longer where a turn commits, only where it ends. What has been said survives a crash in the
    middle of a turn, too (issue [20](20-transactional-effects.md)).
  - **The agent keeps them as they are produced,** not at `commit`. What was not said stays bound
    to the turn's end: changes to notes and memory are applied only by `commit`, as now.
  - **A turn given up** leaves its messages on the active path, the calls that did not run answered
    as not executed (issue [148](148-cancelled-call-without-result.md)), and an entry saying it
    was given up and why: in the channel's history an entry of its own, like `HistoryEntry::Fitted`
    and `Reclassified` (issue [197](197-one-chain-one-model-of-change.md)), in the transcript
    `TranscriptEntry::GivenUp`. `give_up` no longer branches, and `told_of_executed` is no longer
    needed: the agent sees its calls and their results. `TurnFailed` stays as the live event.
  - **Asking an answer under way again** (issue 235) ends it and answers after it, no longer
    instead of it: the attempt stays, marked as given up, and the new turn sees it. Asking a
    finished answer again (issue [234](234-modify-revoke-and-retry-messages.md)) is unchanged - it
    is refused already when tools of the answer ran.
  - **A lowered channel** (issue 141) fits what was said before the lowering, as it fits every
    message it holds (`HistoryEntry::Fitted`); what the turn would say afterwards is refused before
    each step, as now. Since the channel stores each message as it arrives, it admits one only if
    it fits the channel as it is then.
  - **`internal`** keeps its meaning for a plan's steps: not heard by the other participants
    (`Message::is_heard`), and not the answer. It no longer decides what is kept.

  - **The other agents hear the messages of a turn given up** that are not internal: by the
    principle they were said, and they hear them as `as_heard` gives them. The steps of a plan and
    every tool call stay unheard, as now.
  - **A turn given up makes the moderator ask nobody to respond,** not even for a mention in what
    it said: it is no contribution to be answered. Its messages are heard with the next turn that
    is asked for anyway.

## Plan

Written against the code as of `70a53d0`, from a reading of the turn, the two chains, the channel,
the moderator and the web UI. Names of types and functions are proposals for the implementation to
keep or to better; what is fixed is where a responsibility lies and what each step leaves behind.

### A. What the code shows

- **A turn's message is pushed, then published.** `Turn::push_message` appends it to
  `TurnState.entries` with its context and its future transcript digest; the actor then sends it,
  `event_for` choosing `Complete…`, or `Intermediate…` for an internal one (`actor/agent/mod.rs`,
  `actor/mod.rs`). Three places say something: the model's complete message, the tool message of
  a step, and the tool message with which `abandon_tool_calls` answers the calls that did not run
  when the turn ends - pushed inside `commit` and `give_up` and never published; the users get it
  only inside `FullResponseGenerated`.
- **Partial messages are never pushed.** What is being streamed when a turn ends is lost: that is
  the "complete" of the principle.
- **`TurnState` is stored whole after every step** (`store_channel_state`), entries included, and
  resumed after a restart; nothing is republished on resume.
- **The agent's chains take a turn at its end.** `commit` calls `record_turn`: every message into
  the transcript, the not-internal ones into the agent's history with their `ContextRef`, so the
  context of an internal message is recorded nowhere. `give_up` records the same, moves both
  heads back, pushes `TranscriptEntry::GivenUp { end, given_up }` after the head and an internal
  system message (`told_of_executed`) naming the calls that ran - in full, or by name only once
  the channel was lowered below the turn (`names_only`, from `TurnFlow`). Notes and memory are
  applied by `commit` alone.
- **A turn's label is bounded.** `Work::label` joins what the turn has seen *within* the channel's
  classification at its beginning (`join_within`): a message above the channel in the transcript
  raises no later turn's label. When a channel is lowered, its history records `Fitted` and the
  agents' chains keep every message as it was said.
- **The channel writes its history from `FullResponseGenerated`** alone (`append_history`),
  after `admits` asked `Publish` for every message of the event against the channel as it is
  then - which it asks for `Complete…` and `Intermediate…` events already. Users and external
  agents send every message twice, as `Complete…` and inside `FullResponseGenerated`; the chains
  do not deduplicate.
- **The agents hear through `FullResponseGenerated` alone.** The moderator forwards it to the
  agents if a message of it `is_heard`, and asks whom the mentions or the policy name to respond;
  `Complete…`, `Intermediate…` and `TurnFailed` go to the users only; the agents' `event_filter`
  and `take_in` know nothing else.
- **Of the four reasons a turn is given up**, `Failed` and `Lowered` send `TurnFailed { message }`
  to the users, `Retried` and `Stopped` send nothing; and the channel drops what a stopped agent
  still sends, since it left (issue 162).
- **Every reader of a history gets `Vec<Message>`**: the snapshot (the last 20 by index), the
  routes, the views, `catch_up`; `visible_part` cuts by the position of a message. An entry that
  is not a message is invisible to all of them.
- **The web UI** has no branch for `Intermediate…`, removes a turn's messages on a turn-targeted
  `RetryAccepted`, shows `TurnFailed` as a notice a reload loses, and keys the chat by index; the
  `HistoryPanel` folds the agent's given-up turns in at `GivenUpTurn::after`, from
  `GET …/given-up-turns`. The TUI prints intermediate messages as `[working]`.

### B. Decisions

1. **The turn keeps its entries; both ends record them on the active path.** `TurnState.entries`
   stays what a turn's context is built from (prompt parts placed among its messages, issue 49;
   `ContextRef::turn_entries`) and is stored after every step already, so nothing is lost before
   the end. Pushing each message into the transcript as it is produced would fold the entries
   into the chain and change how a turn's context is resolved - a restructuring of its own with no
   gain for the principle, left to a later issue (part J). `commit` and `give_up` differ only in
   what follows the messages: `commit` applies what the turn staged, `give_up` records why it
   ended.
2. **One way to say something in a turn.** `Turn::say(message)` pushes and publishes; the three
   places of A use it, the tool message of abandoned calls included. Everything a turn pushes is
   said, so the channel can store what it is told and nothing exists only in
   `FullResponseGenerated`.
3. **Said is stored; ended is decided.** The channel stores every `CompleteMessageGenerated` and
   `IntermediateMessageGenerated` it admits as it arrives, and nothing from
   `FullResponseGenerated` any more. The agents hear a message as the channel stores it: the
   moderator forwards a `CompleteMessageGenerated` that `is_heard` to the agents, which take it in
   as they took in the messages of `FullResponseGenerated` - between turns, in order.
   `FullResponseGenerated` keeps its payload and becomes the moderator's cue alone: the turn ended
   with an answer, decide whom to ask; it carries the turn's id beside its messages (`None` for a
   user's message, which is no turn), so that the channel records the turn's end from it. The
   alternative - the agents keep hearing from
   `FullResponseGenerated`, and a turn given up ends with an event carrying its messages for
   them - fails for a lowered channel: `admits` would refuse such an event, whose messages carry
   the turn's label, as a whole. Hearing on `Complete…` needs no second path and no exception.
4. **`TurnGivenUp { turn_id, reason }` replaces `TurnFailed`.** One event for every reason, with
   the `GivenUpReason` (`Failed { message }`, `Lowered { channel }`, `Retried`, `Stopped`), sent
   by the agent as it gives up - by the external agent for its endings alike - and routed to the
   users alone; the moderator asks nobody. From it the channel records the turn's end; the web UI
   words it as it words a given-up turn today (`reasonText`).
5. **`Stopped` is the channel's to record.** An agent removed from the channel is gone before its
   run loop gives the turn up, and the channel drops what it still sends. So `remove_participant`
   asks `turn_under_way()` before `stop()` and, if there is a turn, records it as given up
   (`Stopped`) and announces `TurnGivenUp` as it announces `ParticipantRemoved`: the removal is the
   channel's act, as fitting on a lowering is. The agent's own chains record `Stopped` as they
   record every reason.
6. **Every turn's end is an entry, and a history is read as one list.** Both chains get two
   entries, named as the events: `TurnFinished { turn_id, at }` and
   `TurnGivenUp { turn_id, reason, at }`. The transcript's `GivenUp { end, given_up }` is the
   second of them: `end` goes, since the turn's messages precede the entry on the active path. A
   history is read as its entries as they stand, `items()`: one ordered list of `HistoryEntry`,
   of which only `Said` - its message folded, as `messages()` folds it - `TurnFinished` and
   `TurnGivenUp` occur, the changes having gone into the messages; no type of its own for that,
   the doc comment says which variants a reader gets. The snapshot and the history routes carry
   that list in place of `messages`, and paging counts items. So the turn's end is
   in its place for everyone - the web UI folds the steps of a plan when it sees it, after a
   reload too - and `given_up_turns`, `GivenUpTurn`, its `after` and `GET …/given-up-turns` go:
   nothing has to find a turn's messages by their `turn_id` any more. `messages()` stays for what
   wants the messages alone (catching up, the context). Weighed and set aside: a record beside
   the messages as a second list of the snapshot, placed by `after` as the history panel places
   given-up turns today - a second reader for one history; and a record as a message of the
   runtime - a message has an author, a classification and a content, a record has none of them.
   `TurnFinished` in the transcript tells the model nothing and changes the transcript's digest
   at every turn's end, as `GivenUp` does today; the price of reading a turn from either chain
   the same way.
7. **The model is told at resolve time.** `ContextResolver::assemble` words a `TurnGivenUp` entry
   as a system item - why the turn ended, that what its tools did stays done - as it places prompt
   parts: the entry is the fact, the wording its presentation, and a `ContextRef` reconstructs it
   alike. `told_of_executed` and `names_only` go: the calls and their results stand on the active
   path in the turn's own messages, and a turn's label is bounded by the channel's at its
   beginning (A), so a lowered turn's messages raise nothing later.
8. **A turn that said nothing is recorded too**, in both the agent's chains and the channel's
   history: one rule, and the users keep the notice they saw. Today such a turn leaves nothing.

### C. The model (`caelumora-model`)

- `message.rs`: `GivenUpReason` moves here from `transcript.rs`, beside `MessageEntry` and what
  both chains share; `GivenUp` and `GivenUpTurn` go. The comment at `Message::internal` says
  what the flag means now: not heard by the other participants, and not the answer.
- `transcript.rs`: `TranscriptEntry::TurnFinished { turn_id, at }` and
  `TurnGivenUp { turn_id, reason, at }` in place of `GivenUp`; `Transcript::end_turn(entry)`
  pushes one after the head; `given_up_turns` goes. Digest schemes
  `caelumora.transcript-turn-finished.v1` over previous and turn, and
  `caelumora.transcript-turn-given-up.v1` over previous, turn and reason - neither over `at`; the
  stale comment on "the reason and the count" goes.
- `message_history.rs`: the same two variants of `HistoryEntry`, with schemes
  `caelumora.history-turn-finished.v1` and `caelumora.history-turn-given-up.v1`; `fold` and
  `said` pass them over; `end_turn`; `items()` beside `messages()`, both of the active path as
  it stands - `items()` the entries with the changes folded into their messages, so `Said`,
  `TurnFinished` and `TurnGivenUp` alone; `messages_after` and `last_message_id` unchanged.
- `context.rs`: `ContextResolver::assemble` turns a `TurnGivenUp` entry into a system item
  (decision 7) and passes `TurnFinished` over; `usage` is unchanged.
- `agent.rs`: `record_turn` pushes every message of the turn into the history too, each with its
  context - the context of an internal step is recorded from now on; the anchor of a note may be
  an internal message, which the channel's history holds as well - and ends with `TurnFinished`
  on both chains. `give_up_turn` records the entries, moves no head, and ends with `TurnGivenUp`
  on both chains - also for a turn without entries. `retried` passes a `TurnFinished` over as it
  passes a change, and keeps answering `NotLast` for a message followed by `TurnGivenUp`; tested
  now.
- Tests: the branch tests of `transcript.rs`, `agent.rs` and `context.rs`
  (`a_turn_given_up_is_recorded_after_its_branch…`, `…branched_off_after…`,
  `…reaches_the_stored_transcript…`, `a_turn_given_up_is_a_branch_no_longer_active…`,
  `usage_sums…on_every_branch…`) say the active path instead; new: a turn's end is read among
  the items in its place, finished or given up with its reason; a turn without messages leaves
  its end; the digests cover the turn and the reason, not the time; the resolver words a turn
  given up and nothing for one finished.
- **Stored data.** A transcript holding a `GivenUp` entry stored before this change does not read:
  the entry's shape changes, and no reader of the older form is written (AGENTS.md). The agent
  channel states of agents that gave a turn up before have to be deleted - in practice the state
  store's agent channel states, or the sessions. Chains without such an entry read as before;
  digests are kept as recorded and never recomputed.

### D. The runtime: the turn (`agent/turn`, `actor/agent`, `actor/external_agent`)

- `Turn::say(message)` (decision 2): `push_message` and `send_event(event_for(…))` in one; the
  model's complete message, `ToolCalls::collect` and `abandon` go through it, so the tool message
  of abandoned calls is said live. `respond_complete` keeps the emit processors before it.
- `Turn::commit` is unchanged: it hands the not-internal messages on, which
  `FullResponseGenerated { turn_id, messages }` carries for the moderator (decision 3); the user's
  actor and the external agent send it with `None` and the work's id.
- `Turn::give_up(reason)` loses `names_only` and the telling: it abandons the calls (said live),
  records the turn through `give_up_turn`, and returns what it returns today. `because()` moves to
  the resolver's wording; `told_of_executed` and `executed_calls` go.
- `AgentActor::give_up` sends `TurnGivenUp { turn_id, reason }` for `Failed`, `Lowered` and
  `Retried` - after the turn is recorded and stored, so that a reload and the event agree; for
  `Stopped` nothing (decision 5). `TurnFlow` stays for the check before each step.
- The agent's `event_filter` admits `CompleteMessageGenerated`; `take_in` perceives it as it
  perceived the messages of `FullResponseGenerated`, which it no longer takes in (decision 3). The
  external agent's filter and intake alike.
- The external agent's `Ending::Failed` and `Lowered` send `TurnGivenUp` with the reason and the
  work's `turn_id` in place of `TurnFailed`; `Stopped` sends nothing, as today. The channel records
  `Stopped` only for a turn `turn_under_way()` names, which an external agent's never is: an
  external agent stopped while answering leaves no record, as nothing of its answer was complete.
- Tests (`ending.rs`, `actor/agent/mod.rs`, `channel.rs`): the assertions of a branch and of the
  telling become: the turn's messages on the active path, `GivenUp` after them, the model's
  context wording the reason, no telling; `a_turn_given_up_before_it_said_anything_leaves_nothing`
  inverts; `committing_a_turn_answers_with_its_final_messages…` keeps "internal steps are not the
  answer".

### E. The runtime: the channel and the moderator

- `ChannelLoop::append_history`: `CompleteMessageGenerated(m)` and
  `IntermediateMessageGenerated(m)` push `Said { message, context: None }`;
  `FullResponseGenerated` with a turn's id pushes `TurnFinished`, `TurnGivenUp` the entry of its
  name, both with the envelope's time; the messages of `FullResponseGenerated` are stored no
  more. The order of `process_envelope` stays: admitted, mentions resolved, recorded, stored,
  routed.
- `remove_participant`: `turn_under_way()` before `stop()`; with a turn, `GivenUp { Stopped }` is
  recorded and `TurnGivenUp` announced after `ParticipantRemoved`, in the channel's name as that
  is.
- The moderator: `CompleteMessageGenerated` to the users and, if it `is_heard`, to the agents but
  its sender; `TurnGivenUp` to the users; `FullResponseGenerated` to the users and the decision,
  no longer forwarded to the agents; `IntermediateMessageGenerated` and the partials to the users,
  as today.
- `visible_history` and `visible_part` cut items, by position on the path; `ChannelView::history`
  hands the items of the visible part out, a turn's end to every reader of the history alike: it
  carries no classification and no content but the reason the users were told. `catch_up` and
  what builds a context keep reading `messages()`.
- Tests in `channel.rs`: the assertions "nothing of the given-up turn reached the channel" (the
  retry under way, the failed turn after a tool ran, the lowering, the removal while a tool runs,
  the removed agent's idle state) invert - the messages are in the history, followed by the record
  with its reason; a message said after a lowering is refused, the ones before are fitted; the
  tool message of abandoned calls is in the history after a cancel; a user's and an external
  agent's message is stored once. `policy_moderator.rs`:
  `how_an_answer_comes_about_reaches_the_users_only` becomes "a complete message heard reaches the
  agents too"; a `TurnGivenUp` asks nobody. `intake.rs`: heard from `Complete…`, in order with the
  requests. The security suite: a turn's end is read by whoever reads the history, and cut with it
  by a reader's history start; the lowering case, with the permission checks it asks.

### F. REST (`caelumora-rest-server`)

- The channel's stream carries `TurnGivenUp` in place of `TurnFailed`, and
  `FullResponseGenerated` with its `turn_id`. `ChannelSnapshot` carries `items` in place of
  `messages` - the entries as the model serializes them, `Said` with its message, or a turn's end
  with `turn_id`, `at` and its `reason` - the last 20 of them; `GET …/messages` becomes
  `GET …/history` with the same items; the agent's `GET …/actors/{aid}/history` answers items
  too, its messages with `has_context` and `in_transcript` as today.
  `GET …/actors/{aid}/given-up-turns` and `GivenUpTurnResponse` go.
- REST-SERVER.md: the events, the snapshot, the routes.

### G. The web UI and the TUI

- **Live** (`ChatView`): `IntermediatePartialGenerated` streams into the sender's answer as
  `PartialMessageGenerated` does; `IntermediateMessageGenerated` goes into `messages` as
  `CompleteMessageGenerated` does, the workspace version bumped on tool results alike,
  deduplicated by id; `TurnGivenUp` ends the sender's answer and appends the turn's end,
  `FullResponseGenerated` with a turn's id alike; `RetryAccepted` with a turn target removes
  nothing. The chat is a list of items, keyed by message id or turn id.
- **The steps of a plan**: after `mergeToolResults`, the run of internal messages of one `turn_id`
  is one collapsible block in the sender's card - open while the turn is under way, folded once
  the turn's end arrived - each message rendered as today, tool calls included; the pattern of
  `Reasoning`.
- **A turn's end**: a `TurnGivenUp` item in its place, worded by `reasonText`, replacing the
  `turnFailed` notice; its "What the turn did" button goes, the turn's messages stand in the
  chat. A `TurnFinished` item shows nothing of its own; it folds the steps. The messages of a
  given-up turn carry no retry icon: `mayRetry` skips a message followed by a turn's end.
- **`HistoryPanel`**: the same items as the chat, from the agent's history, without the folded
  blocks and `rowsOf`; what was not passed on to the transcript greyed out as today. Internal
  messages in the agent's history have a context to open now.
- **TUI**: prints the reason of a `TurnGivenUp` as it printed `TurnFailed`; `[working]` stays.
- WEB-UI.md: the chat while an agent answers, asking again, the history panel.

### H. Steps

Each builds, passes the tests and leaves the system usable; the documents on the subject change in
the step that changes what they describe.

1. **The web UI shows the steps of a plan live** (G's first two points, without the record): the
   two `Intermediate…` branches, the workspace refresh, the collapsible block of internal messages,
   keys by id. Needs nothing of the runtime; a reload still loses the steps. WEB-UI.md.
2. **One way to say something in a turn** (decision 2, D's first point): `Turn::say` and its three
   call sites. The one change in behaviour: the tool message of abandoned calls is said live,
   which the cancel tests of `channel.rs` see. AGENT-TURN.md.
3. **The chains keep what a turn said** (C, decisions 1, 6, 7, 8): the two entries, `items`,
   `record_turn` and `give_up_turn` on the active path, the resolver's wording; the runtime
   follows where it reads them (`give_up` without a branch, still with its telling; the view's
   `given_up_turns` and its route read from the items until 6). The tests of the model. The
   stored data that has to go, said in the summary. AGENT-TURN.md's chain section.
4. **The turn's end** (D, decision 4): `TurnGivenUp` for the agent and the external agent,
   `told_of_executed` and `names_only` gone; the moderator routes it; REST, web UI and TUI take
   the event in place of `TurnFailed` in the same step - as a notice, not yet stored. The tests
   of `ending.rs`, `actor/agent`, `channel.rs` and the moderator. AGENT-TURN.md,
   CONFIDENTIALITY.md, CHANNELS.md, REST-SERVER.md; ARCHITECTURE.md, CODING-GUIDELINES.md and
   TOOLS-AND-COMMANDS.md where they name the event.
5. **Said is stored** (E, decisions 3, 5): `append_history` on arrival, `Stopped` by the channel,
   the agents hear on `Complete…`, `FullResponseGenerated` with the turn's id for the moderator
   alone and the turn's end recorded from it, the items in the views; the security suite; the
   tests of `channel.rs`, `intake.rs` and the moderator. CHANNELS.md, MEMORY.md. *From here on
   nothing said is unsaid*; the web UI shows a turn's end live and loses it on reload until 6.
6. **The history as items for users** (F, G's third and fourth points): the snapshot and the
   history routes carry items, the given-up-turns route goes, the items in the chat and the
   `HistoryPanel`, the retry that removes nothing. REST-SERVER.md, WEB-UI.md. *From here on the
   issue's "done when" holds.*
7. **Close**: 141, 145, 146 and 235 get an entry under their history; the follow-ups of J opened;
   status `done`.

### I. What to watch

- **One event per message for the agents** instead of one per turn: more pressure on an inbox
  (issue 223). A full inbox loses a message, not a turn, and `catch_up` finds it in the history.
- **A message said after a lowering** carries the turn's label and is refused at the gate, as
  today: it stands in the agent's chains and not in the channel's history, where the record says
  the turn was given up as lowered. The tool message of abandoned calls of such a turn is one of
  them.
- **A crash between the push and the publish of one message** leaves it in the turn's entries and
  not in the channel; a resumed turn republishes nothing. The case of issue 20, unchanged.
- **Order.** The channel's history interleaves a turn's messages with what others said meanwhile;
  the agent's chains hold the turn as a block at its end. The same messages, a different order;
  `missing_from` works by id, and nothing compares positions across the two.
- **The messages of a given-up turn cannot be asked again** (`NotLast`), rightly: nothing is to
  be taken back. Asking the agent to answer after one is a wakeup, or the follow-up of J.
- **`turn_under_way` names a turn only while it is interruptible**: an agent removed while its
  turn reflects or is being cancelled leaves its record in its own chains and not in the
  channel's. Rare, and visible in the agent's history panel.
- **The record of a failed turn carries the failure's message**, as `TurnFailed` did: it names
  what failed and never a secret or a path (CODING-GUIDELINES.md).
- **The snapshot's 20 items count the steps of a plan and the turns' ends**: a page holds fewer
  answers. Paging is issue 218.
- **An external agent's answer given up** leaves only its record, since nothing of it was
  complete; its cancelled answer is taken over as today.
- **Digests**: nothing stored is recomputed; only the given-up entry's shape changes (C). The
  transcript's digest changes at every turn's end from now on, without new input for the model.

### J. Not in this plan

- Answering again after a turn given up: `RetryTarget::Turn` of a turn that ended, accepted as a
  new answer after it without taking anything back - an issue of its own; until then, a wakeup.
- Folding a turn's messages into the transcript as they are produced (`TurnState.entries` gone), a
  restructuring of how a turn's context is resolved; and reducing `FullResponseGenerated` to the
  turn's id once nothing reads its messages but the moderator.
- Paging (218); erasing an attempt (237); the recorder (05); the TUI beyond one line per record;
  records in the agent's context panel.

## Done when

- Every complete message of a turn - its answer, a plan's steps, their tool calls and results - is
  in the channel's history and in the agent's transcript and history on the active path, as soon
  as it is said, also when the turn is given up; changes to notes and memory are still applied only
  when the turn ends well.
- Every turn's end - finished, or given up with its reason - is an entry of the channel's history
  and of the agent's chains, read among the messages in its place; `give_up` no longer branches.
- The other agents hear the messages of a turn given up that are not internal; a turn given up
  makes the moderator ask nobody to respond.
- Asking an answer under way again keeps the attempt; asking a finished one again, revoking,
  reclassifying, lowering a channel and paging the snapshot work with the messages kept.
- The web UI streams `IntermediatePartialGenerated`, shows `IntermediateMessageGenerated` live,
  refreshes the workspace after a step's tool results, shows the steps of a plan collapsible, a
  turn given up marked with its reason, and no longer removes an attempt that was asked for again.
- AGENT-TURN.md, CHANNELS.md, CONFIDENTIALITY.md and MEMORY.md describe it; the issues whose
  decisions this changes (141, 145, 235) say so under their history.
- The agent channel states of agents that gave a turn up before this change are deleted (part C,
  *Stored data*); nothing else stored changes.

## History

- **2026-10-08** — Opened after the maintainer noticed that no messages arrive in the web UI
  while a plan is executed. Decided at first that `FullResponseGenerated` carries the whole turn,
  internal messages included, and the history keeps storing on it alone. Then the maintainer
  widened it to a principle - nothing said is unsaid, also when a turn is given up - so that what
  is kept no longer depends on how a turn ended; the channel stores each message as it arrives.
  Two questions were left open and then decided with the maintainer as recommended: the other
  agents hear what a turn given up said, if it is not internal; and a turn given up makes the
  moderator ask nobody to respond.
- **2026-10-08** — Planned with the maintainer, against the code as of `70a53d0` (parts A to J).
  Decided in the plan: the turn keeps its entries and both ends record them on the active path;
  one `Turn::say` for everything a turn says; the channel stores what it admits as it arrives and
  the agents hear on `CompleteMessageGenerated`, `FullResponseGenerated` being the moderator's cue
  alone; `TurnGivenUp { turn_id, reason }` replaces `TurnFailed`, and the channel records
  `Stopped` itself; the record is an entry of both chains, read beside the messages with `after`;
  the model is told at resolve time, `told_of_executed` goes; a turn that said nothing is recorded
  too. The "done when" on stored data is amended: transcripts with a turn given up before do not
  read and are deleted.
- **2026-10-08** — Decision 6 changed at the maintainer's proposal: not a record beside the
  messages as a second list, but every turn's end as an entry of the history - `TurnFinished` and
  `TurnGivenUp` - and a history read as one list of items. `given_up_turns`, `GivenUpTurn` and
  `GET …/given-up-turns` go with it; `FullResponseGenerated` carries the turn's id.
