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

## History

- **2026-10-04** — Issue created at the maintainer's request, who saw Jonathan "talk to himself" at
  the end of a quiz with Hermes. Reported first as an agent mentioning itself; the data showed no
  self-mention - the moderator and the channel keep an agent from asking itself - but a backlog of
  requests worked through one turn each. The maintainer recalls that requests used to be coalesced
  in the agent: they were, until `3220226` (2026-07-16) took events one at a time - a regression.
