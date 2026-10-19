# 223 — An actor whose inbox is full loses what was said, silently

**Status:** open
**Type:** bug
**Priority:** high
**Area:** Sessions and channels
**Summary:** An actor whose inbox is full - an agent in a long turn in a busy channel - loses what was said, requests and approvals without anyone learning of it.

## The problem

The channel delivers to each actor with `try_send` into a queue of 100 envelopes
(`ChannelLoop::spawn_actor`, `route_event_impl`). When the queue is full, the envelope is dropped
and a warning is logged ("Queue full for actor ..."); nothing else happens.

An agent reads its inbox only between turns (see [AGENT-TURN.md](../docs/AGENT-TURN.md), *the
cognitive cycle*), and while it waits for an approval it takes only approvals - putting the rest
into the receiver's own buffer, which has no bound. While its model answers, nothing is read at
all. Since issue [222](222-queued-requests-answered-one-by-one.md) everything queued is taken in at
once when the turn ends, which makes a long backlog harmless to the conversation - but only what
was queued: what did not fit is gone. Of what reaches an agent, its event filter lets through
approvals, `FullResponseGenerated` and `ResponseRequested`, so a busy channel - each message one of
the former and often one of the latter - fills the queue after some 50 messages during one long
turn, e.g. a slow local model working through a plan.

What is lost:

- **A message heard:** missing from the agent's history and transcript for good. The agent answers
  without it, and nothing makes it catch up later - `catch_up` runs only when an actor is built.
- **A request:** not answered - harmless after 222 if a later one comes through, lost otherwise.
- **An approval:** the turn waits for a decision that never arrives, until its timeout - the
  approver thinks it delivered it.

The other actors are affected alike, the user's actor reading its inbox to feed a frontend
included, but an agent's turn is what holds a queue up for minutes.

## Why it matters

- Silent loss: neither the agent nor the users learn that something was not heard; a warning in the
  server log is all.
- It grows with what 222 is about: the slower an agent's model and the busier the channel, the more
  likely - exactly the agents that already lag behind.

## Considerations

- **A larger queue** only moves the limit.
- **An unbounded queue** loses nothing, but an actor that never reads - hung in a tool, say - grows
  without bound. The channel would have to notice and act (end the actor's turn, drop it).
- **Waiting (`send` instead of `try_send`)** loses nothing, but one slow actor then holds up the
  channel's routing for everyone - rejected unless per actor.
- **Catching up instead of queueing what was said.** A heard message need not travel through the
  queue at all: it is in the channel's history, and `catch_up` (issues 190, 196) already takes in
  what an actor's history misses. Dropping a `FullResponseGenerated` could then be repaired by
  catching up before the next turn - requests and approvals still need a way that never drops.
- **Reading the inbox during a turn** - keeping it drained into the receiver's buffer, as
  `recv_matching` already does while waiting for approvals - is the unbounded queue by another
  name.

## Done when

- No envelope an actor needs is lost without the actor or the channel doing something about it -
  what was said is caught up, a request or decision is not dropped.
- A test with an agent whose turn outlasts more envelopes than its queue holds shows that it hears
  all of them afterwards.

## Plan: the inbox with the history behind it

Decided with the maintainer on 2026-10-19, in place of the plan of 2026-10-16 (see *History*),
together with [385](385-inbox-lost-on-restart.md). The concept in one sentence: **an actor's
inbox is a bounded queue of live envelopes, and behind it the channel's history, which the inbox
reads from wherever the queue fell short.** Nothing important is lost, because everything
important is recorded; nothing is stored twice, because the history is the one store; and an
actor keeps nothing but where it stands in the history.

The mental model the maintainer chose: a live envelope carries the `SecurityContext` its sender
set when it sent it - what was true as the request was given. An envelope read from the history
instead carries the initiator as recorded and the rest as the sender is *now*. That is less exact,
and it is the fallback alone: on an overflow, after a restart, on a join. The one place that
reconstructs a context is the channel, and the one check it reaches is an agent's of a command
(`AgentBody::carry_out`, "E,3" in [PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md)).

### 1. The history records everything important, with its context

- **`Recorded` carries the envelope's head** (`caelumora_model::history`): beside `entry`,
  `sender` and `recipients`, the `EventContext` of the envelope as it was delivered - sender and
  name, event, correlation and span ids, span depth, agent hops, initiator - and its `timestamp`.
  `Recorded::new` takes them from the envelope. `sender` stays what it is - whose word the entry is,
  the system's for a request to respond, whose context the request carries - since `seen_by`
  gives a sender what it sent. Session, channel and position are the chain's; the position is
  stamped as the entry is read.
- **`HistoryEntry::event`** builds the `ActorEvent` back from an entry: the inverse of
  `Envelope::entry`, in the model, with a test that the two round-trip. A message said comes back
  as an intermediate one if it is internal - a step of a plan, as an agent sends it - and as a
  complete one otherwise, so no rebuild is lossy. `Fitted` gives no event: it is none.
- **`TurnFinished` gets its answer back from the chain.** The entry names the turn alone; the
  messages the event carries - the turn's answer, what it said that is not internal, for the
  moderator to decide whom to ask (issue 258, step 5b) - are the `Said` entries of the turn
  before it, each recorded as it was delivered. **`HistoryStore::answer(channel, turn_id,
  before)`** gives them, oldest first: it reads the chain backwards from `before` and stops at the
  previous end of a turn of the same actor - `TurnFinished` or `TurnGivenUp` -, since an agent's
  turns do not overlap, or at the chain's start. Nobody is asked whose turn it is: the one the end
  is delivered to was delivered its messages. `HistoryEntry::event` gives the end with no
  messages, and the channel, which has the store, fills them in as it builds the envelope
  (`recorded_for`, below) - the one event whose rebuild reads beyond its entry. The same query is
  what [261](261-turn-finished-repeats-the-answer.md) found for the moderator, should the event
  one day carry no copy of the answer.
- **`Recorded::envelope(seq, security)`** builds the envelope: the event, the recorded context with
  `position: Some(seq)`, the timestamp, and the `SecurityContext` the caller gives - the model
  does not know who the sender is now.
- **What is recorded anew** - what an actor's state depends on receiving, and the history does not
  hold today: `CommandRequested { agent, command, args }`, `RetryRequested { target, author }`,
  `ApprovalRequested { request }`, `ApprovalDecided { response, status }` - granted, denied and
  timed out as one entry with its `ApprovalStatus` -, `ApprovalRequestWithdrawn { id }`,
  `QuestionRequested { request }`, `QuestionAnswered { id, agent, responses }`,
  `QuestionDeclined { id, agent }` and `QuestionClosed { id, shown_to }`: nine variants,
  `Envelope::entry` and `HistoryEntry::event` an arm each. They are **no items** (`is_item`
  false): in the chain for delivery alone, never in a page, in `StandingItems` or in a
  `HistoryItemResponse` - the clients are shown them live, as today, and nothing changes for the
  web UI. Everything else that is not recorded is transient - streamed chunks, `StateChanged`,
  `ResponseStarted`, `CommandStarted`, `ActivityStarted`, `PlanStepFinished`, `WakeupRequested`,
  `NudgeRequested` (which becomes a recorded `ResponseRequested`) - and may be lost in the
  fallback: what each announces the start of is recorded at its end.
- **Whom they went to** is a rule like every entry's - `Envelope::concerns` states it by kinds and
  by name: the approvers, the agent that asked, the users and that agent - and `Recipients` says
  it once it names kinds and names together: one struct, `Recipients { users, agents, approvers,
  named }` - every user and agent that was a member at the time, as the ranges say, the channel's
  approvers, and these by name, member or not -, in place of the enum `Members { agents } |
  Only(..)`, which it covers (`Recipients::members`, `Recipients::only`, `Recipients::NOBODY`). An
  approver is the channel's own actor and no participant (issue 420): it has no range, so an entry
  to the approvers reaches them without one. An outsider sees what went to every user.
  `HistoryEntry::recipients(sender)` takes the sender, which the approval entries go by, as
  `Recorded::new` has it. A question shown to some alone goes to them by name, as a command's
  answer does. No `Option`, no resolution by the channel, and no marker value that must never be
  stored.
- **One rule, not two.** `concerns` and `recipients` state the same rule today, for every
  recorded event, in two places; with nine more they would do so twice over. `Envelope::concerns`
  becomes `Envelope::recipients`: for a recorded event its entry's rule, for the rest - chunks,
  states, the start of a step, a command or an activity, approvals and questions until step 2
  records them - its own; what only asks goes to nobody, as now. The channel computes it once per
  event and asks it of each actor (`Recipients::names`). A refactoring without a change of
  behaviour, before the nine entries (step 1).
- **`Heard`** (`caelumora_model::history`): where a reader of a history stands - `next`, the first
  position not delivered to it in order, and `taken`, the positions after it delivered ahead of
  their turn (an approval taken while a turn waits, `recv_matching`). `took(seq)` moves `next`
  past `seq` and forgets what `taken` holds before it; `took_ahead(seq)` adds to `taken`. One
  small type with one responsibility, used by the inbox, which keeps it, and by the agent's state,
  which stores it. `AgentChannelState.heard: Heard` is stored with everything the agent took in,
  in the same put (below). `last_message` stays: the notes and the memory anchor at it.
- **`HistoryStore::entries`** returns `Vec<Node<Recorded>>`, the entry as the reader may see it
  (`as_seen_by`) with the recorded head: what the inbox builds envelopes from. Both stores
  (`JsonFileStore`, `MemoryStore`) change in that one method. **Every stored state (`state/` of the
  JSON store) is to be deleted:** the chain's entries and the agent channel states have a new
  required field each.

### 2. The inbox: a queue, and the history behind it

**`Inbox`**, in `caelumora-runtime/src/channel/inbox.rs`, one per running actor, opened by the
channel (`ChannelLoop::open_inbox`) and shared by its two halves under one lock, with a `Notify`:

- **The sending half** is the channel's (`RunningActor.inbox`): `deliver(envelope)` applies the
  actor's `event_filter` - `FilteringEventSender` goes -, and queues the envelope if there is room
  (`Inbox::CAPACITY`, 100 as today). If the queue is full, a recorded envelope (`context.position`
  is `Some`) is dropped and the inbox marked `behind: Some(position.next())` - the history has it
  from here on -; a transient one is dropped and logged at debug, as the channel did at warn. While
  behind, every recorded envelope is dropped and `behind` moved past it, every transient one
  dropped: the queue holds everything before the first position dropped, the history everything
  from it on. `close()` is what dropping the sender did: the receiver ends. `TransportError::Full`
  and "Queue full" are gone.
- **The receiving half** is the actor's, its `EventReceiver`: `recv` hands out the queue first,
  oldest first, and moves `heard` with each (`took`). Once the queue is empty and the inbox is
  behind, it reads the history from `heard.next` on - as the actor
  (`Reader::Participant`), through `ChannelLoop::recorded_for(actor, from)`, which drops what the
  actor sent itself unless it is an announcement (the rule of `route_event_impl`), builds each
  envelope (`Recorded::envelope`, with the sender's context as it is now, below; a turn's end with
  its answer from `HistoryStore::answer`) and stamps it as the live path stamps - one function for
  both paths -, and the inbox applies the filter and
  skips `taken`. What it read ahead it keeps until handed out, a page at a time, never more: a
  cache of the history, re-read if need be, not a buffer. When a read finds nothing more and, under
  the lock, `heard.next` has reached `behind`, the inbox is no longer behind, and the channel
  queues again - race-free, because every drop moves `behind` under the same lock, after its entry
  was stored. `recv_matching`, `try_recv` and `try_recv_if` work on the same sequence - queue,
  then history -; `recv_matching` searches it and takes ahead (`took_ahead`), leaving the rest
  where it is, in the queue or in the history: the unbounded buffer of
  `InMemoryEventReceiver::recv_matching` is gone. `try_recv` and `try_recv_if` become `async`,
  since the history is a store read. `commit` is replaced by `heard() -> Heard`: there is nothing
  to commit to, the actor stores where it stands with its state.
- **The sender's context as it is now**: `Actor::security_context()` joins the `Actor` trait -
  every actor has one (`UserActor`, `ApproverActor`, `AgentBody`) - and the channel asks the
  sender's actor for it; a sender that takes no part any more is given a context without rights
  (`SecurityContext::new`, no roles), and the initiator is the recorded one. Stated in
  PERMISSION-CHECKS.md as what "E" means for an envelope read from the history. The inbox reaches
  the channel as the actors' runtimes do (`ChannelHandle`, a `Weak<ChannelLoop>`); a channel that
  is gone closes the inbox.
- **Where it begins.** The channel opens the inbox at a `Heard`: for an actor that joins, at
  `from`, where its view begins (`HistoryShared`); for one that starts again with its channel, at
  what its state says (`Actor::heard()`, `async`, `None` for an actor that keeps nothing: at the
  history's end, nothing behind); for one that rejoins with a state, at `from` or `heard.next`,
  whichever is later, since what it heard before is in its transcript already (387's rule). An
  inbox opened before the end is behind from the start: a join and a restart are the overflow
  case, nothing of their own. `Actor::catch_up` goes, and with it `AgentBody::catch_up`,
  `ActorRuntime::channel_entries` and `channel_position_of`.
- **The order of a join** (`apply_change`): the inbox is opened and put among `running` first,
  then `HistoryShared` and `ParticipantJoined` are announced - recorded, and dropped by the inbox
  as it is behind -, then the actor's task is spawned. So the join is recorded before the newcomer
  reads, its membership range is derived, and it reads as a participant: `Reader::Newcomer` goes
  (issue 423's gap between catching up and the inbox cannot open, since there is no catching up).
- **Delivering** (`route_event_impl`) changes in one thing besides `deliver`: a `RetryRequested`
  for a turn is checked - under way, or the last given up - before it is recorded, in `enter`,
  rather than as it is delivered: what is not delivered is not recorded, so that the fallback
  cannot deliver it. `reject` delivers through the inbox like everything else.

### 3. The actors: taking in writes where they stand

- **Taking in moves the mark.** `AgentBody::take_in`, `take_in_during_turn` and
  `AgentActor::handle_waiting_for_user` end with `state.heard = rx.heard()`: the mark is written
  into the state by taking in, as the messages are, so that every store that follows - the run
  loop's after each step, `carry_out`'s, `end_turn`'s, the external agent's after `hear` - carries
  it. No call site needs to remember it at store time. The calls of `commit` go.
- **Nothing is taken in twice, by construction.** What a stored state holds and where its `heard`
  stands are one put. After a restart the channel delivers from `heard.next` on, minus `taken`:
  exactly what the stored state does not have. A store that failed leaves the stored mark where
  the stored state is, so what the memory had beyond it comes again. The idempotent taking in the
  old plan needed is not needed; neither is `History::position_of`.
- **A turn that waits** (`WaitingForUser`) takes decisions and answers ahead of their turn from
  the queue or the history alike; what waits behind them stays where it is, for the turn's end.
  `request_pending` stays: the approver keeps its pending requests in memory and starts empty, so
  an agent that resumes asks again; a decision given and not yet taken in is in the history and
  comes from there.
- **Users and the approver** keep nothing and start at the end. A user's actor that falls behind
  loses chunks and states while it catches up - the frontend is shown the complete messages from
  the history - and drops what it is shown twice by id as before.
- `InMemoryEventReceiver` and `InMemoryEventSender` stay for the actors' outputs and for tests;
  the receiver keeps a `Heard` through the same two methods, for `heard()`.

### 4. The tests

- **223's:** an agent whose turn outlasts more envelopes than the queue holds - two hundred
  messages, a modification, a command - hears all of them afterwards, in order, and carries the
  command out, in the agents' channel tests.
- **385's:** a channel stopped while an agent is in its turn and started again: a change to a
  message, a request to respond and a decision on an approval that were delivered and not stored
  are taken in once; what was stored is not taken in again.
- **The inbox** (`inbox.rs`): the queue in order; an overflow marks it behind and the history is
  read from `heard.next`; `taken` is skipped; the inbox stops being behind only once the read
  passed every drop; `recv_matching` from the history; a closed inbox ends.
- **A join**: a newcomer with a `from` before the end hears what it was given, then its join,
  then what is said from there on, nothing twice and nothing missed - the test of issue 423,
  rewritten without `catch_up`.
- **The model**: `Envelope::entry` and `HistoryEntry::event` round-trip; the new entries are no
  items and go to whom the channel names; `Heard::took` and `took_ahead`.
- **The store suite**: a `Recorded` with its head round-trips through the chain; `entries` gives
  the head with the entry as seen; `answer` gives a turn's messages, not its internal ones and
  nothing of the actor's previous turn, also across a segment.

### What is documented

- CHANNELS.md: *An actor's inbox* and *Actors and envelopes* - the queue, the history behind it,
  the fallback's context -; *How much history a newcomer sees* without catching up: the inbox
  opens at `from`.
- STORAGE.md, *Histories*: `Recorded` with its head; the entries that are no items; `Heard` in an
  agent's state.
- AGENT-TURN.md, *Between turns*, *Hearing during a turn* and *Storing and resuming*: taking in
  moves the mark; what a restart delivers again.
- PERMISSION-CHECKS.md: "E" for an envelope read from the history.

### Steps

1. The model, restructured: `Recorded` with its head, `HistoryEntry::event`, `Recorded::envelope`,
   `Heard`, `AgentChannelState.heard`, `HistoryStore::entries` over `Recorded` and
   `HistoryStore::answer`; `Recipients` by kinds and names, `Envelope::recipients` by the entry's
   rule; the tests of the model and the store suite. **Every stored state is to be deleted.**
2. The model, extended: the nine entries, no items, each with its `recipients`; `Envelope::entry`
   and `HistoryEntry::event`.
3. The runtime: `Inbox` and its tests; the `EventReceiver` trait; `Actor::security_context`,
   `Actor::heard`, `catch_up` removed; `open_inbox`, `recorded_for`, the stamping shared; the
   order of a join; the retry check moved; `Reader::Newcomer` removed; the channel's tests.
4. The agents: the mark written by taking in; `commit` and `catch_up` removed; the agents' channel
   tests, 223's and 385's.
5. The documents; 223 and 385 `done`, 387's note and 431's mention of the buffer resolved.

## History

- **2026-10-04** — Issue created while resolving [222](222-queued-requests-answered-one-by-one.md):
  taking in everything queued at once makes a backlog harmless, but the queue a backlog waits in
  drops what does not fit.
- **2026-10-06** — Since [234](234-modify-revoke-and-retry-messages.md), what a full inbox drops may
  also be a change to a message - `MessageModified`, `MessageRevoked` - or a request to answer
  again. A dropped change is lost for good: catching up (`MessageHistory::missing_from`) finds
  messages an actor lacks, not changes it missed, so its store keeps the old version while the
  channel's shows the new. Whatever this issue decides for messages has to hold for changes as well.
- **2026-10-08** — Since [258](258-what-was-said-is-lost.md), an agent hears what others say as
  each message is said (`CompleteMessageGenerated`) rather than once per answer: more events per
  inbox, and a full inbox now drops a message rather than an answer. Catching up finds dropped
  messages as before; the decision whom to ask still comes once per turn, with its end.
- **2026-10-16** — Taken up as step 2 of the epic [378](378-resource-management-epic.md), with
  [218](218-paged-histories.md) done. Catching up from the history was rejected - the moderator
  decides who hears what -, and the maintainer proposed persisting the inbox instead, which also
  answers [385](385-inbox-lost-on-restart.md), opened for the loss on a restart. The plan above
  was written for both - and then both were taken out of the epic: the plan needs no range reads
  of the history, only an entry by position, and reduces nothing held in memory. It waits for
  [379](379-history-stored-as-it-stands.md), whose store contract has to say how a history entry
  and the deliveries it causes are stored together once the history has a store of its own.
- **2026-10-16** — [379](379-history-stored-as-it-stands.md) is done. Its contract answers the
  question the plan above waited for: the put of a `ChannelState` writes the history's new entries
  and then the document - in Postgres in one transaction, in the JSON store in that order, with what
  a crash leaves past the head not there - so the inboxes, as part of the document, are stored with
  the entry that caused their deliveries. The channel holds only a window of its history, so
  `ChannelState::envelope_of` reads an entry before the window from the `HistoryStore`
  (`entries`).
- **2026-10-16** — From [387](387-agent-history-window-without-compaction.md): an agent's actor
  finds where to take in what it missed when it joins (`catch_up`) by the last message of its
  history the channel has, though the history is informational and never to decide what reaches
  its transcript. The position up to which an actor has heard its channel, which this issue needs,
  would answer that as well; `catch_up` is to read it once it exists.
- **2026-10-19** — Weighed with the maintainer, not decided yet: since
  [417](417-user-history-from-its-actor.md) every entry of the history records whom it went to, as
  the moderator decided, so the reason the plan above gives for rejecting catching up from the
  history - a reader of the history would hear everything - no longer holds for what is recorded.
  An inbox could be a position in the history, read as the actor (`Reader::Participant`), and a
  small queue for what the history does not record; the bound, the gap and the deliveries per actor
  would go. That needs envelopes built back from the history, which
  [432](432-envelope-carries-whole-security-context.md) settled: an envelope keeps the
  `SecurityContext` its sender's actor set when the request was given, and the history is the one
  place a security context is built again - for an envelope built back when an actor joins or its
  channel starts again after a pause. It records the initiator (`EventContext::initiator_id`),
  which cannot be built again; the rest comes from the sender as it is then, so that a request
  taken up after a pause is checked with the rights of then - a deliberate exception, to be stated
  in the plan.
- **2026-10-19** — Decided with the maintainer, and the plan above written in place of the plan of
  2026-10-16: the inbox stays a bounded queue of live envelopes, which carry the context their
  sender set - the model the maintainer chose: what held as the request was given holds -, and
  the history is what the inbox reads from wherever the queue fell short, on an overflow, after a
  restart and on a join alike. So the history is the one store: no `Inbox` in the channel's state,
  no `Delivery`, no `Gap` and no bound by kind; what is important is recorded - commands, retries,
  approvals and questions join the history, as entries that are no items -, and the rest is
  transient. An actor keeps where it stands (`Heard`), stored with what it took in, so that
  nothing is taken in twice without any taking in having to be idempotent. The maintainer's
  concern that the inbox filters, and that an agent takes approvals ahead of their turn, is what
  `Heard::taken` is for. The fallback is the channel's, in the inbox it opens, not the actors':
  `catch_up` goes.
