# 385 — An actor's inbox is lost on a restart

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** Sessions and channels
**Summary:** An actor's inbox is a queue in memory, so a restart loses what was routed to it and not yet taken in - a change to a message, a request, a command - for good.

## The problem

An actor's inbox is a queue in memory (`mpsc::channel(100)` in `ChannelLoop::spawn_actor`). When
the server stops - a restart, a crash - whatever was routed to an actor and not yet taken in is
gone with it. Of what an agent misses that way, two things are repaired when the channel starts
again: what was said, which `catch_up` reads from the channel's history (issue 190), and the
approvals its turn waits for, which it asks for again (`request_pending`, issue 172). Everything
else is lost for good: a message modified or taken back while the agent was in its turn (issue
234) - its own history keeps the old version while the channel's shows the new -, a request to
respond, a request to answer again, a command it was asked to carry out.

`EventReceiver::commit` exists for exactly this - "persistent transports: advance offset / send
ack" - and both kinds of agent call it once their state is stored, but no transport is persistent:
the in-memory receiver's `commit` clears a list.

## Why it matters

- A restart while an agent works through a long turn is the likeliest moment for a backlog, and
  the backlog is what is lost.
- A change to a message is lost silently: the agent's history diverges from the channel's, and
  nothing notices.
- The same queue drops what does not fit while the server runs
  ([223](223-full-inbox-drops-events.md)): one design has to answer both.

## Considerations

- **The inbox as part of the channel's state**, delivered to and stored with the history entry
  that caused the delivery, so that a crash between the two cannot lose one without the other;
  taken in by the actor through an `EventReceiver` the channel makes, whose `commit` records how
  far the actor got. What was delivered and not committed is delivered again when the channel
  starts, so taking in has to be idempotent.
- **Content once.** What is recorded in the channel's history - a message said, a change to one -
  is stored in an inbox as its position there, not again; the inboxes of every actor share the one
  copy. What is not recorded - an approval, an answer to a question, a request - is stored as it
  is. What is transient - streamed chunks, state changes - goes to actors that keep nothing and
  need no stored inbox.
- **A bound, with a policy** for what exceeds it, decided by the maintainer: oldest dropped or
  newest ignored, or by the kind of the delivery - the whole of
  [223](223-full-inbox-drops-events.md)'s question.

## Done when

- A channel that starts again delivers to each actor what was delivered to it and not committed
  before the stop, and an agent takes it in once.
- A test shows it for an agent whose turn outlasted a change to a message and a request.
- STORAGE.md and CHANNELS.md say what an inbox is and where it lies.

## History

- **2026-10-16** — Opened while weighing options on [223](223-full-inbox-drops-events.md): the
  maintainer proposed persisting the inbox, which answers this loss and 223's with one design. The
  design is 223's plan; this issue is done with it.
- **2026-10-16** — Taken out of the epic [378](378-resource-management-epic.md) with 223, which
  records why; after [379](379-history-stored-as-it-stands.md).
- **2026-10-19** — Decided with [223](223-full-inbox-drops-events.md), whose plan of 2026-10-19
  replaces the one of 2026-10-16 and answers this issue without a stored inbox: the queue stays in
  memory, and what it held that was not yet stored is delivered again from the channel's history,
  from where the actor's stored state says it stands (`Heard`), since everything important is
  recorded there - commands, retries, approvals and questions from then on too. Nothing is taken
  in twice, because the mark and what was taken in are one put; the taking in need not be
  idempotent. Done with 223's step 4, which holds this issue's test.
- **2026-10-19** — Done with [223](223-full-inbox-drops-events.md): an agent stores where it stands
  in its channel's history with what it took in, and its inbox opens there when it runs again.
  The test: a message, the request to answer it and a change of what was said, delivered while the
  agent was in its turn and not stored, are taken in once after a restart. The answers to
  questions are no part of the history; a turn resumed asks again.
