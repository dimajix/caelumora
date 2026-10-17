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

## Plan: the inbox as part of the channel's state

Decided with the maintainer on 2026-10-16, together with [385](385-inbox-lost-on-restart.md): an
actor's inbox is persisted, bounded by a limit far above today's, and holds what is recorded in the
channel's history once, as a position there. Catching up from the history in place of delivering
was weighed and rejected: the moderator decides who hears what (`Moderator::route`), and a policy
other than `route_said`'s may well deliver a message to some agents and not to others; a reader of
the history would hear everything. Delivering stays; what changes is where the queue lies.

### 1. The model: an inbox is a queue of deliveries

- **`Inbox`**, in `caelumora_model::channel`: the deliveries to one actor in order, each at a
  running position of the inbox's own, and `committed`, the position up to which the actor took
  them in (`EventReceiver::commit`). A `Delivery` is an envelope's head - its timestamp, security
  and event context - with a `Payload`: `Recorded { at }`, the position in the channel's history
  of the entry the event is (a message said, modified, taken back or reclassified); `Event(…)`
  for what the history does not hold - an approval, an answer to a question, a request to respond
  or to answer again, a command; `Gap { dropped }` for what the bound threw away (below). The
  inboxes of every actor share the one copy of what is recorded; a delivery is a few hundred
  bytes whatever the message is.
- **`ChannelState.inboxes: HashMap<ActorId, Inbox>`**, beside `history`, with a change
  `ChannelStateChange::Inbox { actor, change }` - `Delivered(Vec<Delivery>)`, `Committed(u64)`,
  `Removed` - so that the journal carries deliveries and commits as it carries history entries.
  `ChannelState::envelope_of(&Delivery) -> Option<Envelope>` builds the envelope back: a
  `Recorded` one from `history.at(at)` - `Said` becomes `CompleteMessageGenerated`, `Modified`
  `MessageModified`, and so on -, an `Event` one as it is. An entry at a position never changes,
  so the envelope is the one that was routed; a change made since arrives as a delivery of its
  own, after it.
- **The bound.** `Inbox::LIMIT`, 10 000 deliveries - a constant; a setting can follow when
  someone needs one. Above it, the oldest deliveries whose payload is `Recorded` are dropped and
  counted in one `Gap` at their place (merged with a gap next to it): what was said is in the
  history still, and the gap tells the actor how much it missed. Requests to respond collapse into
  the last - `take_in` answers only the last anyway. Approvals, answers to questions, requests to
  answer again and commands are never dropped and do not count: each is one deliberate act of a
  person, few by nature, and dropping one leaves a turn waiting or an order undone. *Weighed:* one
  policy for all - oldest dropped, or newest ignored, as the maintainer proposed. Newest ignored
  loses the freshest request, which is what the actor needs next; oldest dropped may hit the
  approval a turn waits for. By kind it is.

### 2. The runtime: the channel delivers into the state and stores once

- **Delivering** (`ChannelLoop::route_event_impl`): no `try_send` into a queue of 100. The
  moderator's decision is computed as today, each target's `event_filter` applied as today, and
  then, under the state lock, the history entry and every delivery are pushed and the state is
  stored once (`append_history` and the deliveries become one step): a crash cannot store the one
  without the others. `TransportError::Full` and "Queue full" are gone.
- **Waking**: a `tokio::sync::Notify` per running actor (`RunningActor`), notified after the
  store. The `mpsc` queue is gone.
- **`InboxReceiver`**, in `caelumora-runtime/src/channel/inbox.rs`, the `EventReceiver` the
  channel gives its actors: it holds the channel's state and store, its actor's id and how far it
  delivered. `recv` hands out the next delivery after that, resolved through
  `ChannelState::envelope_of`, or waits on the notify; `recv_matching` and `try_recv` as the
  in-memory receiver does them, over the same deliveries; `commit` sets the inbox's `committed` to
  how far it delivered and stores the state. `None` once the inbox is removed or the channel stops.
  `Actor::run` and `EventReceiver` do not change; `InMemoryEventReceiver` and `InMemoryEventSender`
  stay for the actors' outputs and for tests.
- **Kept or not.** An actor that keeps nothing - a user's, the approver's - gains nothing from a
  stored inbox and is sent a stream of chunks that would fill the journal for nothing. `Actor`
  says whether its inbox is kept (`inbox_kept`, both agent actors; the default is not); the inbox
  of an actor that keeps none is the same `Inbox` in the `RunningActor`, in memory, with the same
  bound and a `commit` that stores nothing. Its `Recorded` deliveries still resolve through the
  history: no copy of a message per user actor either.
- **Joining and leaving**: `spawn_actor` makes the receiver so that it delivers from the inbox's
  `committed` on - after a restart, what was delivered and not committed is delivered again.
  `ActorChange::Stop` removes the inbox (`Removed`) and closes the receiver, as closing the
  `mpsc` does today. `catch_up` stays what it is: the way in for what was said before an actor
  joined, or while its state was lost; the stored inbox makes it unnecessary for a restart, which
  it does not notice.

### 3. The actors: taking in once

- **Idempotent**: a delivery not committed is delivered again after a restart, so `take_in`
  skips a message its history has (`History::position_of`), and a change is applied only where it
  changes something - `History::modify`, `revoke` and `reclassify` already say whether it did,
  and are to be checked for recording a change that changed nothing. The transcript the same.
- **A gap** is taken in as a message of the system - "N messages were not delivered to you" -
  perceived as everything heard, so that the model knows, and logged as a warning. The alternative,
  a log line alone, leaves the agent answering as if nothing were missing - what this issue is
  about.
- **Users and the approver** commit nothing today; with an inbox in memory they need not. The
  frontends drop what they are shown twice by id (REST-SERVER.md, *Snapshots*).

### 4. The tests

- An agent whose turn outlasts more deliveries than the old queue held - two hundred messages, a
  modification, a request - hears all of them afterwards, in order (this issue's *done when*).
- A channel stopped while an agent is in its turn and started again: what was delivered and not
  committed - a change to a message, a request - is taken in once (385's).
- The bound: deliveries over the limit become a gap the agent takes in; an approval over the
  limit is kept.
- The store suite: a `ChannelState` with inboxes round-trips through the journal, deliveries and
  commits.

### What is documented

- CHANNELS.md: what an inbox is, that the channel delivers into its state, what is kept, the
  bound and the gap; the bullet on *an actor's inbox*.
- STORAGE.md: `ChannelState` holds the history and the inboxes; who writes which part - the loop
  the deliveries, the receiver the commits - and that both lock the one state.
- AGENT-TURN.md: the gap, and that taking in is idempotent.

### Steps

1. The model: `Inbox`, `Delivery`, `Payload`, `ChannelState.inboxes`, the change, the bound and
   its policy; `envelope_of`; tests. **Every stored state (`state/` of the JSON store) is to be
   deleted.**
2. The runtime: delivering into the state, the `InboxReceiver`, the notify, kept and unkept
   inboxes, joining and leaving; the channel's tests.
3. The actors: idempotent taking in, the gap, `inbox_kept`.
4. The tests of step 4 above, in the channel's and the agents' suites.
5. The documents; 223 and 385 `done`, 378's order noted.

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
