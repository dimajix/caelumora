# 502 — Two subscribers of the event bus lose events without catching up

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** Sessions and channels
**Summary:** The `ControllerManager`'s follower and the `ApproverActor` receive the bus through `EventSubscription::recv`, which logs lost events and goes on, so a controller keeps an outdated value and pending approvals are not checked again after a join or a leave that was lost.

## The problem

A subscription to the `EventBus` holds `SUBSCRIPTION_CAPACITY` (128) events; one that falls further
behind loses the oldest. `EventSubscription::recv` (`caelumora-runtime/src/event.rs`) notes the loss
in the log and returns the next event - meant for subscribers "that have no way to catch up". Two
subscribers that do have one use it all the same:

- **The follower of the `ControllerManager`** (`start_follower`,
  `caelumora-runtime/src/controller/manager.rs`) takes the updates of agents, users, profiles,
  workspaces and projects into their live controllers. It subscribes to `All`, so every event of
  the instance counts towards its 128 - a burst of messages (`channel::SAID`) or of agent states is
  enough. An update it lost is never taken in: the controller answers with the old value - an
  agent's name, a user's profile, a workspace's access list - until it is loaded again.
- **The `ApproverActor`** (`caelumora-runtime/src/actor/approval.rs`, `run`) follows its channel's
  topic and checks its pending approvals again when someone joins or leaves, and denies those of an
  agent that left. A lost `channel::LEFT` leaves the approvals of an agent that is gone pending
  until they time out; a lost join or leave leaves the others decided by who took part before.

The other subscribers catch up already: a running session looks at all its agents again
(`follow_agents`), the `CommandReaper` takes a loss as a reason to check (`check_commands`), and
the server-sent event streams send a fresh snapshot (issue
[38](38-global-stream-loses-events-silently.md)).

## Why it matters

- What the controllers hold is what the runtime acts by; an outdated access list of a workspace is
  a decision on outdated rights.
- With several instances ([122](122-several-instances.md)) every subscriber must survive a gap -
  between processes loss is the rule, not the exception -, so each needs a way to catch up anyway.

## Considerations

- **Catch up by reading again.** Each takes `RecvError::Lagged` from the receiver itself, as the
  session and the reaper do, and reads what it keeps anew from the store: the follower every live
  controller of its registries - workspaces and projects re-read already on every update -, the
  approver `recheck_all_pending` and its channel's participants.
- **Narrow what the follower receives.** It subscribes to `All` for want of topics for workspaces,
  projects and user profiles (issue [346](346-subscription-topics-for-every-kind.md)); with topics
  by kind it would count only the events it wants. That makes loss rarer, not impossible - catching
  up is needed either way.
- **`recv` that logs and goes on.** Once no subscriber is left that cannot catch up, the method has
  no user and goes; the tests that read the bus have no need for it. To be checked with the other
  callers of `recv` - `SessionController::process_global_event` is fed by the session, which follows
  only its agents after a loss.

## Done when

- The follower and the approver catch up after lost events, a test each shows it.
- No subscriber of the bus outside the tests skips lost events without catching up, or says why it
  may.

## History

- **2026-10-29** — Opened from a review of the event bus with the maintainer: of the subscribers
  inside the runtime, the follower and the approver are the two that skip lost events.
- **2026-10-29** — Done, branch `issue-502-bus-subscribers-catch-up`, as decided with the
  maintainer:
  - **The follower** is a type of its own, `Follower` in `controller/manager.rs`, which takes an
    event into the live controller it is about (`take`) and, after a loss, reads every live agent,
    user, workspace and project controller again (`catch_up`), through a `reload` of each. A
    user's controller now holds the store of users for it.
  - **Reading again under the lock**, decided: `LiveDocument::take_stored` reads the document
    again while holding the lock of a change, so that a change of this instance under way is not
    set back by what was read before it. The workspace's and the project's `process_global_event`
    read again through it too, which had the same gap; `Writing::change` keeps `reload`, holding
    the lock already.
  - **The approver** takes the receiver itself and makes up for a loss (`catch_up`): the pending
    requests of every agent that has no actor in the running channel are denied, the others decided
    again.
  - **The session**, decided to be done here: after a loss it followed its agents again
    (`follow_agents`), but not the topic of an agent taken in whose announcement was lost, so that
    agent's deletion or restoring would not reach it. It now follows every agent taking part after
    a loss (`agent_topics`). Its own document it does not read again, as "Done when" asks to
    say why: a session runs in exactly one instance, and its controller there is the one writer of
    it, which holds every change live as it stores it; the events about it that a loss drops are
    its own.
  - **`EventSubscription::recv` and `try_recv`**, which skip a loss, are built for tests alone
    (`test-support`), decided: no subscriber of the runtime can skip one unnoticed any more, which
    the compiler holds.
  - **The approver's catch-up** denies the requests of an agent that is no longer a participant
    of the channel document, or rests in it without an actor: the document is changed before the
    leave is announced, the actor taken out after, so either alone would leave a window.
  - **Reading user and profile** as stored is one function, `UserController::load_stored`, which
    the `ControllerManager` makes a controller from and `reload` reads again from.
  - A test each shows the follower (agents, users, profiles, workspaces, projects), the approver
    and the session making up for a lost announcement, each failing without the change; another
    that reading again waits for a change under way. They make a subscriber fall behind with
    `EventBus::publish_past_capacity`, a helper of the tests that publishes more than a
    subscription holds.
