# 222 — A slow agent answers every request queued while it worked, one stale answer each

**Status:** open
**Area:** Sessions and channels

## The problem

An agent takes its inbox one event at a time (`AgentActor`, `actor_impl/agent.rs`): a message heard
is perceived, a `ResponseRequested` begins a turn, and while a turn runs nothing is taken in - the
events wait (see [AGENT-TURN.md](../docs/AGENT-TURN.md), *the cognitive cycle*). When the turn ends,
the next event is taken, and if it is the next request, the next turn begins - on the conversation
as perceived so far, which is one message more than before.

An external agent does the same (`ExternalAgentActor::run`, `actor_impl/external_agent.rs`): it
takes one event at a time, and a request waits until the remote agent has answered
(`respond`). It asks the remote agent only when it has heard something new since the last call -
but since it takes what it heard one message at a time too, nearly every queued request has one new
message before it, and the remote agent is called once per request.

An agent that answers more slowly than the others are asked thus falls behind and then works
through its backlog: every message that asked for it gets a turn of its own, one after the other,
each answering one more message that is already overtaken. Seen on 2026-10-04 in a channel of Kaya,
Jonathan (gemma4, 20-60 s a turn) and Hermes (external, about 10 s), local time:

| Time | Kaya | Hermes | Jonathan |
|---|---|---|---|
| 20:50:38 | last message: "I'll leave you to it and watch" | | |
| 20:51 | | 20:51:20 | 20:51:09, 20:51:55 |
| 20:52 | | 20:52:08 | 20:52:14, 20:52:30, 20:52:50 |
| 20:53 | | 20:53:02, 20:53:30 | 20:53:15, 20:53:57 |
| 20:54 | | 20:54:00 - his last | 20:54:24 |
| 20:55 | | | 20:55:15, 20:55:56 |

Ten messages of Jonathan in five minutes against five of Hermes, and two more after Hermes had
stopped; he repeats himself ("Dann wird nicht länger gewartet!", "Dann wird jetzt nicht länger
herumgedruckst!") and at 20:55:56 still awards Hermes the point for an answer given two minutes
before, addressing Hermes and Kaya, who had both stopped. To the people in the channel he seems to
talk to himself.

The same happens on a smaller scale when two agents answer the same message at once and mention
each other: each answers the other's message afterwards, which its own answer has overtaken.

## Why it matters

- The channel fills with answers to a conversation that has moved on, and a slow model makes it
  worse the longer the others talk.
- Every stale turn costs a model call, and in a chain between agents a hop
  ([45](45-agent-mentions.md)) - the chain ends at the limit with stale answers.

## Considerations

The moderator decides who is asked to respond; the actor decides how it answers. The backlog is the
actor's, so the actor is where it is resolved, not the moderator - which does not know whether an
agent is busy, and must not drop a request whose message would then go unanswered.

- **Coalescing in the actor - as it was.** Until 2026-07-16 the agent did this: it took everything
  queued at once (`recv_many` into a buffer, up to 100 events), perceived every message heard, and
  began one turn if any of them was a request, in the context and security of the last envelope;
  events arriving during a turn were kept and taken the same way afterwards
  (`take_pending_events`). `3220226` ("Refactor transport channels") replaced the buffer by the
  `EventReceiver` - `recv` one event at a time, `commit` after it - and with it the batching; the
  pending events went with `2c59a8a` before. Restoring it means taking what the receiver already
  holds without waiting, and committing once for all of it.
- **Coalescing in the actor.** Before a turn begins on a request, the actor takes in everything
  that is queued behind it: it perceives every message heard and drops every further request, since
  the one turn answers the conversation as it stands. A request that arrives during the turn leads
  to one turn afterwards - not one per request - and only if something was heard after the turn
  began. The prompt cache is unaffected; the agent sees every message, in order.
- **Stale requests dropped.** A request is dropped when the message that asked for it was already
  in the context of a turn the agent completed. Exact per request, but the transcript and the
  request have to be matched by the message (the envelope's span or the message id).
- **The moderator does not ask a busy agent.** It would have to know the agents' states, and what
  is said while an agent works would never be answered by it - rejected unless combined with one of
  the above.
- **Which events may be taken in ahead.** Approvals and cancels already have their own order during
  a turn ([148](148-withdrawing-approvals.md)); only heard messages and requests are coalesced, and
  a request that carries a different security context (another user asked) may need a turn of its
  own.
- **Both kinds of agent** have the backlog (see above): one rule for both, in the code they share
  where possible - how an actor takes its inbox is the same question for both (issue 206 made what
  they hear the same).

## Done when

- An agent that falls behind - run here or external - answers the conversation as it stands once it
  is free again, instead of once per request that was queued; nothing it was asked about goes
  unheard.
- Tests with a slow agent of either kind and several requests queued while it answers show one
  answer afterwards.
- AGENT-TURN.md describes how queued requests are taken.

## Implementation plan

Coalescing in the actor, as it was, on the `EventReceiver` as it is now. Nothing of the moderator or
the channel changes.

### What the code does today

- The moderator delivers a message as `FullResponseGenerated` to everyone first, then the
  `ResponseRequested` to whom it asks (`PolicyModerator::route_full_response`). The inbox of a busy
  agent therefore holds `[F1, R1, F2, R2, ...]`: every request has the message that asked for it
  right before it.
- `AgentActor::handle_waiting` takes one event with `rx.recv()`, perceives an `F`, begins a turn on
  an `R`, and commits. During the turn nobody reads the inbox.
- `ExternalAgentActor::run` takes one event in its `select!` the same way; `exchange` asks the
  remote agent whenever the transcript holds something not given yet, which after every `F` it does.
- `EventReceiver` offers only the waiting `recv` and `recv_matching`. `commit` already means
  "everything delivered since the last commit", so it fits a batch as it is.

### Steps

1. **`EventReceiver::try_recv(&mut self) -> Option<Envelope>`** (`transport.rs`): what is there
   already, without waiting - the counterpart of `EventSender::try_send`. `InMemoryEventReceiver`
   takes from its buffer first (what `recv_matching` put aside while a turn waited for an
   approval), then from the queue with `mpsc::Receiver::try_recv`, and keeps what it hands out as
   uncommitted like `recv` does. `None` says nothing is queued now, not that the input is closed;
   that is still `recv`'s to tell. A step of its own, without a change of behaviour.
2. **`take_in` in `actor_impl/mod.rs`**, next to `perceive` and `catch_up` and with their
   parameters (`state`, `takes`, `processors`, `security`), plus the event already received and the
   receiver. It takes that event and everything `try_recv` still yields, in order: the messages of
   every `FullResponseGenerated` go through `perceive`, one call per event, so that a failing
   perceive stage costs the transcript one message, not the batch; of the `ResponseRequested` it
   keeps the last. Everything else is dropped, as both actors drop it between turns today. It
   returns the request to answer, `Option<Envelope>` - no new type. It neither commits nor stores:
   when that happens differs between the two actors.
3. **`AgentActor::handle_waiting`** calls `take_in` with the event `recv` returned, then begins at
   most one turn on the request (`request_response`, unchanged) and commits once. Whether the agent
   sleeps is read from `self.control.state()` *after* taking in, not from the `asleep` parameter,
   which goes: the channel wakes an agent before it delivers, and with the message and its request
   taken in one batch, a value from before `recv` would have the woken agent drop the request. The
   span of the step is that of the first event while taking in, and that of the request for
   beginning the turn.
4. **`ExternalAgentActor::run`**: the `rx.recv()` branch calls `take_in` (through its `perceive`
   filter, `worth_giving`), stores the state once if anything was heard - what `hear` does per
   event today, which `take_in` replaces - and calls `respond` at most once, then
   `follow_open_tasks` and `commit` as today. `exchange` needs no change: it gives all the
   transcript holds, which is now all that was queued.
5. **Tests.**
   - `transport_impl/memory.rs`: `try_recv` yields what was put aside before what is queued,
     nothing when both are empty, and what it yielded is committed with the next `commit`.
   - `actor_impl/mod.rs`: `take_in` perceives every message heard in order - those after the last
     request too - and returns the last request; without a request, `None`.
   - `channel.rs`, with the slow model that exists there: an agent asked three times while it
     answers gives one answer afterwards, whose context holds all three messages. The same for an
     external agent with a slow remote: one call afterwards, given all three.
   - `channel.rs`: a sleeping agent that is mentioned answers, also when the message and the
     request reach it in one batch.
   - `security_suite.rs`: a turn begun on several queued requests acts with the security context
     of the last one (see below).
6. **Documents.** AGENT-TURN.md, *the cognitive cycle*: "Perceive" and "Begin" describe that the
   agent takes everything queued at once and begins one turn on the last request; the same sentence
   for the external agent where it is described. `EventReceiver::try_recv` in the document that
   describes the transport.
7. **Record.** Status `done` here and in the index, a History entry, and the new issue of the full
   queue (below).

### Why no further rule is needed

- *One turn afterwards, not one per request:* everything queued during a turn is taken in the next
  time the actor waits, and at most one turn begins on it.
- *Only if something was heard since the turn began:* a request is in the queue only behind the
  message that asked for it, so a turn after a turn always has something new. An external agent
  whose filter left nothing answers with nothing, as today.
- *Nothing goes unheard:* every message is perceived, in order, before the turn's context is built;
  only requests are dropped. The prompt cache is unaffected.
- *Approvals and cancels* keep their order: approvals are taken with `recv_matching` during a turn,
  a cancel does not come through the inbox. A late approval between turns is dropped, as today.
- *After a crash* nothing changes: the commit follows the beginning of the turn, as today, and a
  turn under way is resumed from the stored state.

### Whose authority the one turn has

A request carries the `SecurityContext` of the message that asked for it; its `initiator_id` is the
user at the start of the chain. The requests of a backlog differ in it only when several users
talk.

- **A - the last request's (decided, as it was until July).** The turn always acts for someone
  who did ask; a dropped request means that this user's authority is *not* used, never that
  somebody's is used who did not ask. That the turn sees what others said holds for every turn
  already. The cost: after Alice, who may more, and then Bob, the turn acts as Bob may, and a tool
  Alice could have had run is refused. The test in the security suite states the rule.
- **B - one turn per initiator (not taken).** Exact per user, but the second turn has heard nothing
  new and answers what the first did - the problem again, in small - and `take_in` has to keep a
  request per initiator and the actors a queue of them.

The maintainer decided for A on 2026-10-04, which is what the steps above describe.

### What this does not solve

- **Two agents answering at once that mention each other** still answer each other's message once
  each: it did arrive after their turn began. The limit on hops bounds it. Whether an agent should
  answer a message its own answer has overtaken is a question of the moderator's, not of the inbox.
- **A full inbox loses events.** The channel delivers with `try_send` into a queue of 100 and drops
  with a warning when it is full (`ChannelLoop::route_event_impl`); the actor empties it only
  between turns. A very long turn in a busy channel thus loses what was said - to be opened as an
  issue of its own with step 7.

## History

- **2026-10-04** — Issue created at the maintainer's request, who saw Jonathan "talk to himself" at
  the end of a quiz with Hermes. Reported first as an agent mentioning itself; the data showed no
  self-mention - the moderator and the channel keep an agent from asking itself - but a backlog of
  requests worked through one turn each. The maintainer recalls that requests used to be coalesced
  in the agent: they were, until `3220226` (2026-07-16) took events one at a time - a regression.
- **2026-10-04** — Analysed in the code and an implementation plan added (*Implementation plan*):
  `EventReceiver::try_recv`, and a shared `take_in` next to `perceive` that both actors take their
  inbox with. Found on the way: the `asleep` flag of `handle_waiting` must be read after taking in,
  or a woken agent drops its request; and a full inbox drops events, which becomes an issue of its
  own. Decided by the maintainer: the one turn has the authority of the last request (A), not a
  turn per initiator. Nothing implemented yet.
