# 508 — The event bus queues what it has yet to deliver without a bound

**Status:** in progress
**Type:** feature
**Priority:** low
**Area:** Observability and operations
**Summary:** What is published waits for the event bus's dispatcher in a queue without a bound, so when the dispatcher falls behind, memory grows and every subscriber waits longer, and nothing tells.

## The problem

`EventBus::publish` hands an event to its dispatcher through an unbounded `mpsc` channel
(`EventBus::queue`, `caelumora-runtime/src/event.rs`); one task delivers what it is handed, in that
order (`Dispatcher::run`). Since issue [504](504-bus-has-two-delivery-paths.md), every subscriber -
the runtime's own too - is served on that one path.

Delivering an event asks the policy of every subscription in someone's name that follows one of its
topics (`may_reach_each`), and the ownership policy reads a group the event names from the store
(issue [503](503-bus-asks-every-stream.md): once per question, not once per subscriber). That is a
file read with the JSON store, a round trip with Postgres. While it waits, everything published
after it waits too. When events are published faster than they are delivered - many agents speaking
at once, a slow store - the queue grows without a bound:

- memory grows with it, and is given back only once the dispatcher catches up;
- every subscriber - the controllers, a running session, an approver, the streams of `GET /events` -
  receives later and later;
- nothing measures the queue, and nothing in the log says that the bus is behind.

What a subscriber cannot keep up with is bounded already: each subscription is a broadcast channel
of `SUBSCRIPTION_CAPACITY`, and a subscriber that falls behind loses the oldest and makes up for
them (issue [502](502-bus-subscribers-lose-events-silently.md)). The queue before the dispatcher has
no such bound.

## Why it matters

- A bus that falls behind delays what the runtime acts by - a controller taking in a change, an
  approver hearing of a leave - and what users see, for as long as it is behind.
- Memory that grows unseen ends the process rather than slowing it.
- With several instances ([122](122-several-instances.md)) the events of the others enter the same
  queue.

## Considerations

- **A. Measure first.** The length of the queue and how long an event waited in it, logged when they
  pass a threshold - a warning that the bus is behind, with how far. Nothing changes in delivery;
  it shows whether B or C is needed, and when.
- **B. A bound, and a loss told.** `publish` is not `async` (`EventPublisher`), so it cannot wait
  for room; a full queue would have to drop. A dropped event has to reach every subscriber as a
  loss, so that it makes up for it as for a `Lagged` - which a broadcast channel cannot be told
  from outside; the subscription would need a loss of its own beside the channel's. Bounds memory
  at the cost of a resynchronisation for every subscriber.
- **C. Deliver faster.** What makes the dispatcher slow is the policy's reads: a cache of
  memberships ([287](287-memberships-cache.md)) or an index of the subscriptions by actor and role
  (503) shortens each event. Makes falling behind rarer, not impossible.
- **C2. Ask for several events at once.** The policy questions of the events taken from the queue
  together run at the same time, and the events are delivered in their order as their answers come
  (ordered concurrency, as `buffered(n)` of a stream). Up to n times the throughput without a
  cache, at the cost of n reads of the store at once - and of the subscribers being read when an
  event's question starts, not when it is delivered, so that a subscription made or a topic
  followed meanwhile misses events it is promised now.
- **D. Deliver to the runtime's subscribers apart.** They are not asked about, so they need not wait
  for the policy questions of others; a second task for them would give them back the immediacy
  they had before 504, at the cost of the one order 504 made.

Recommendation: A first; B, C or C2 once the measurements say the bus falls behind in practice.

## Done when

- The queue of the bus is measured, and a bus that falls behind says so in the log.
- What follows from the measurements is decided and recorded here.

## History

- **2026-10-29** — Opened at the maintainer's request, from the review of the event bus that led to
  issues 502 to 504: the unbounded queue before the dispatcher was a finding of 503 and 504, left
  out of both.
- **2026-10-30** — Decided for A, and C2 added to the considerations, to be weighed with B and C
  once the measurements are in. Measuring is done: every event carries when it was published, and
  the dispatcher measures at each it delivers how long it waited (`Backlog`). The bus falls behind
  when an event waited longer than `BEHIND_AFTER` (a second) and others wait after it, and has
  caught up once none waits. It warns when it falls behind, with the wait and how many more wait,
  again every `BEHIND_TOLD_EVERY` (30 seconds) while it stays behind, so that a queue that never
  catches up does not fall silent, and says when it caught up, with the longest wait; falling
  behind again sooner than that after the last warning is told only if it lasts, so that a bus at
  the edge does not fill the log. What the measurement cannot show: a dispatcher held up by one
  event - a store that does not answer - is seen only once it delivers the next. EVENTS.md states
  it. What follows from the measurements is still to be decided.
