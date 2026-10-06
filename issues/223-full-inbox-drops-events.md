# 223 — An actor whose inbox is full loses what was said, silently

**Status:** open
**Area:** Sessions and channels

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

## History

- **2026-10-04** — Issue created while resolving [222](222-queued-requests-answered-one-by-one.md):
  taking in everything queued at once makes a backlog harmless, but the queue a backlog waits in
  drops what does not fit.
- **2026-10-06** — Since [234](234-modify-revoke-and-retry-messages.md), what a full inbox drops may
  also be a change to a message - `MessageModified`, `MessageRevoked` - or a request to answer
  again. A dropped change is lost for good: catching up (`MessageHistory::missing_from`) finds
  messages an actor lacks, not changes it missed, so its store keeps the old version while the
  channel's shows the new. Whatever this issue decides for messages has to hold for changes as well.
