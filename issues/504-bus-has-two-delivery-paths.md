# 504 — The event bus delivers by two paths in no common order

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** The event bus has two kinds of subscription - by topic, delivered at once and unchecked, and as someone, delivered later by a dispatcher and checked - so its subscribers receive the same events in no common order, and two ways of subscribing exist for one concept.

## The problem

`EventBus` (`caelumora-runtime/src/event.rs`) keeps two sets of subscribers and two ways to them:

- **By topic** (`subscribe`, `subscribe_all`, `subscribe_session`, `_channel`, `_agent`,
  `_user`, `follow`): the runtime's own subscribers - the follower of the `ControllerManager`, the
  `CommandReaper`, a running session, the `ApproverActor`. `publish` sends to them at once, without
  a check.
- **As someone** (`subscribe_as`): the streams of `GET /events`. `publish` hands the event to a
  dispatcher task, which asks the policy and sends later.

So the two sets receive an event at different times, and nothing orders the one against the other.
In one process that is harmless. Once events come from other instances
([122](122-several-instances.md)), they enter the bus at one place and are to reach every
subscriber in one order, and loss and catching up (issue
[502](502-bus-subscribers-lose-events-silently.md)) are to be one mechanism. Besides, a
subscription as someone cannot name topics (`follow` does nothing for it), and a topic subscription
cannot be made in someone's name: issue [346](346-subscription-topics-for-every-kind.md), option D.

## Why it matters

- One concept - receiving the events one may read and wants - is two types of subscription with two
  paths, and every change of delivery is made twice.
- A bus between instances needs one entry and one order.

## Considerations

- **One subscription: a context and an interest.** Every subscription is made in a context and
  names what it follows; the runtime's subscribers subscribe as the system, which the policy admits
  to everything without asking. All are served on one path, in the order of publishing.
- **The interest is 346's topic.** Generic topics, `(EntityKind, id)` derived from the event's
  fields with `within`, and a whole kind (346, B and C), make the fixed `SubscriptionTopic` and the
  `subscribe_<kind>` methods unnecessary. 346 decides the shape; this issue the one path.
- **Delivery at once or by the dispatcher.** Today a topic subscriber has an event when `publish`
  returns; through a dispatcher it has it a moment later. To be checked which code and which tests
  rely on the former - tests read with `try_recv` right after publishing. Serving the subscribers
  as the system at once and the others later would keep two orders, so the one path is the
  dispatcher's.
- **Restructure only.** No change of who receives what; issue [503](503-bus-asks-every-stream.md)
  changes how a party is decided, and is independent of this.

## Done when

- The bus has one kind of subscription, made in a context with an interest, served on one path in
  the order of publishing; `docs/EVENTS.md` describes it.
- The tests of the bus pass unchanged in what they deliver.

## History

- **2026-10-29** — Opened from a review of the event bus with the maintainer, as the step before a
  bus between instances (issue 122).
