# 504 — The event bus delivers by two paths in no common order

**Status:** done
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
- **2026-10-29** — Decided with the maintainer: one subscription, `subscribe(context, topics)`,
  served by the dispatcher; the runtime's subscribers subscribe as the system, which the bus does
  not ask about - some policies of the tests refuse the system, and the runtime's subscribers are
  not checked today either; tests that read what they caused wait for the dispatcher by a mark sent
  through its queue rather than for a while; the shape of the topics stays 346's.
- **2026-10-29** — Done, branch `issue-504-bus-has-two-delivery-paths`:
  - `EventBus::subscribe(context, topics)` is the one way to subscribe; `subscribe_as` is gone, the
    REST server's stream subscribes to `All` in the user's name. `subscribe_all`, `_session`,
    `_channel`, `_agent` and `_user` stay as the system's shortcuts until 346. `follow` works for
    every subscription, in someone's name too.
  - The bus keeps one list of subscriptions - context, topics, sender - instead of a map of topics
    and a list of parties. The dispatcher, started with the first subscription, delivers each
    event to the subscriptions following one of its topics (`SubscriptionTopic::of`, what
    `dispatch` computed before): the system's at once, the others once `may_reach_each` was asked
    of all of them together. A subscription whose receiver is gone is dropped as the next event is
    delivered, whatever its topics - the topics' empty entries that stayed in the map before are
    gone with it.
  - The dispatcher takes what is queued at once (`recv_many`), so that a burst is handed on in one
    go, as it was by `publish` before; the tests of 502, which make a subscriber fall behind by a
    burst, rely on it.
  - For tests: `EventSubscription::settled` sends a mark through the dispatcher's queue and waits
    for it, and `EventSubscription::drain` reads what was delivered after it; the synchronous
    `session::testing::drain` and the bus tests' timed one are gone. The bus's tests run in 0.02 s
    rather than 0.6 s.
  - Tests: the runtime's subscription and one in someone's name receive three events in one order;
    one in someone's name follows a topic, and still only what it may read; a dropped subscription
    is dropped, the system's as anybody's. All other tests pass unchanged in what they deliver.
- **2026-10-29** — After the review, on the same branch:
  - **The dispatcher is a type**, `Dispatcher` (private): the subscriptions and whom it asks,
    with `follow`, `deliver` and `run`, the task's loop. The bus holds it and the queue to it;
    the free `deliver`, the helper `locked` and the policies passed alongside went into it.
  - **A subscription holds the bus's sender weakly**, by which `follow` finds it: a subscription
    kept whole learns once the bus is gone (`Closed`), as one whose receiver was taken does - which
    only the subscriptions as someone did before.
  - **`subscribe_agent` and `subscribe_user`** had no caller and are gone; `subscribe_all`,
    `_session` and `_channel` are documented as the system's.
  - **Two small changes of delivery**, both named: whoever subscribes, or follows a topic, receives
    what the bus has yet to deliver - also what was published a moment before, as a subscription
    as someone did before, where one by topic received only what was published after it -; and a
    subscription following two topics an event reaches receives it once, not twice. No subscriber
    of today follows two such topics, and receiving an event published a moment before is what
    the runtime's subscribers expect of it.
  - Tests: the runtime's subscriptions are not asked about - under a policy refusing everybody
    they receive everything, someone's nothing -, failing without the exception; a subscription
    kept whole learns once the bus is gone.
