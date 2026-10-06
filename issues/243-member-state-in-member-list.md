# 243 — The member list does not show what an agent is doing

**Status:** done
**Area:** User interaction

## The problem

The members panel of the web UI lists a channel's participants, but not what an agent is doing:
whether it is idle, asleep, answering, waiting for an approval. Stopping one agent means finding
its answer in the chat (issue [235](235-retry-of-an-answer-under-way.md), step 2); an agent that
works without showing anything - a long tool call, a model that thinks before its first chunk -
looks the same as one that does nothing.

## Why it matters

In a channel with several agents the members panel is where one looks to see who is busy. The
maintainer's wish: a coloured dot per agent for its state, and a cancel icon on an agent that
works.

## Considerations

- **The states.** An agent's actor knows its state (`Actor::state`, an `ActorState`): `Idle`,
  `Sleeping`, and in a turn `Planning`, `ExecutingTools`, `WaitingForApproval`, `Reflecting`,
  `Cancelling`. For users, fewer would do - e.g. idle, asleep, working, waiting for an approval -
  with the turn's states folded into "working". An external agent has a state of its own
  (`set_processing`: `Planning` while it answers, else `Idle`).
- **The snapshot** can carry it as [240](240-answers-under-way-not-in-snapshot.md) carries the turn
  under way: read from the running channel at no cost, `None` where the channel does not run.
- **Keeping it up.** Today the users learn of a turn's start and end (`ResponseStarted`,
  `FullResponseGenerated`, `TurnFailed`) and of approvals, but not of falling asleep (the sleep and
  stop tools) or being woken. Either the panel derives what it can from the events it has and
  reloads on what it lacks, or the agent announces its state - an `ActorEvent` of its own, sent by
  the run loop as it changes state, routed to the users like `ResponseStarted`. The second is
  exact, but every step of a turn would send one; folding to the users' few states would send far
  fewer.
- **The cancel icon** calls `POST …/participants/{aid}/cancel` (`ChannelLoop::cancel_actor`, 235
  step 2), offered where the viewer may cancel the channel - a `can_cancel` the channel snapshot
  does not carry yet; the stop button is offered to everyone today and refused by the server.
- Users and approvers have no state worth showing; the dot is for agents.

## Plan

Decided with the maintainer.

### Keeping it up: the actor announces its state

- **`ActorEvent::StateChanged(ActorState)`**: an actor tells the users where it stands now, sent by
  the actor through its run's transport like `ResponseStarted`; the sender is the envelope's. The
  moderator routes it to the users. The raw `ActorState` is sent, no type of its own: the client
  folds it into what it shows.
- **Where it is sent.** An agent's run loop announces at the top of each step what changed since
  it last announced (`RunControl::unannounced`): every transition of the loop - `settle` and the
  `go_to` after a step - passes there. It speaks in its own contexts, not in a turn's: a state is
  the agent's, not part of an answer (`Answering::own`, which the external agent's unasked speech
  uses too).
- **Waking.** The channel moves a sleeping agent to `Idle` itself, synchronously
  (`RunControl::wakeup`), because the moderator, deciding right after whom to ask, leaves the
  sleeping out. The run loop does not see it while it waits in `handle_waiting` - and after
  `/wakeup` no message follows. So `wakeup` also notifies the run loop (`tokio::sync::Notify`, only
  if it did wake it), and `handle_waiting` waits for the next message *or* that notice, on which
  it announces and waits on. The channel changes the state; the actor announces it.
- **An external agent** announces in `respond`, as it begins and ends an answer (`Planning`,
  `Idle`).
- `Actor::state` - which exists - gives the start: the snapshot.

### Snapshot and cancel

- `MemberSnapshot::state: Option<ActorState>`, an agent's from the running channel at no cost
  like `turn_under_way`; `None` for a user or an approver, a former member, and in a channel that
  does not run.
- `MemberSnapshot::can_cancel`, built like `can_retry`: `CancelChannel` of the channel, of an
  agent taking part - an external one too, whose `cancel` ends its remote task. The stop button
  of an answer under way uses it, offered to everybody before.
- `RunControl::interruptible` becomes `ActorState::interruptible()` in the model (`Planning`,
  `ExecutingTools`, `WaitingForApproval`): a cancel ends only those. The cancel icon is offered
  where `can_cancel` holds and the state is one of them.

### The web UI

A dot per agent in the members panel, its state as a tooltip:

| dot | state |
|---|---|
| green | `Idle` |
| green, pulsing in colour and size | working: `Planning`, `ExecutingTools`, `Reflecting`, `Cancelling` |
| amber | `WaitingForApproval` |
| blue, still | `Sleeping` |
| grey | absent or broken: the channel does not run, the agent is gone |

Seeded from the snapshot, kept up by `StateChanged`. A working agent gets a cancel icon (`POST
…/participants/{aid}/cancel`). Users and approvers get no dot.

## Done when

Every agent in the members panel shows its state as a coloured dot, kept up while the page is open,
and an agent that works can be stopped from there.

## History

- **2026-10-06** — Opened on the maintainer's request while
  [240](240-answers-under-way-not-in-snapshot.md) was implemented.

- **2026-10-06** — Planned with the maintainer: the actor announces its state
  (`ActorEvent::StateChanged`), a wakeup notifies its run loop to do so; `state` and `can_cancel`
  per member in the snapshot; the colours above.
- **2026-10-06** — Done, as planned. `ActorEvent::StateChanged(ActorState)`, routed to the users
  like `ResponseStarted`. `AgentActor` announces as its run starts - so that a client that came
  before the channel ran learns it - and at the top of every step what moved since
  (`RunControl::unannounced`, over a second `ProcessingStateHolder`, which got `swap`);
  `RunControl::wakeup` notifies the run loop through a `Notify` when it did wake the agent, and
  `handle_waiting` announces on that and waits on. `Planning` comes right after a turn's
  `ResponseStarted`, which the waiting step sends. `ExternalAgentActor` keeps its state in a
  `ProcessingStateHolder` now, like the agent run here, and announces as it starts and in `go_to`.
  Both speak in `Answering::own`, which the external agent's unasked speech uses too.
  `ActorState::interruptible`; `AccessQuestions::may_cancel_channel`; `MemberView::may_cancel` and
  `MemberView::state`; `MemberSnapshot` and `ChannelMemberResponse` carry `can_cancel` and `state`.
  The web UI shows the dot, a stop icon on an agent a cancel can still end, and the stop icon of an
  answer under way only where `can_cancel` allows it. Tests: the run control tells of a wakeup only
  if it woke the agent, and announces each state once; an agent tells where it starts, its turn
  (`Planning`, `ExecutingTools`, `WaitingForApproval`, then `ExecutingTools`, `Reflecting`,
  `Sleeping`) and that `/wakeup` woke it; an external agent tells `Planning` before its answer and
  `Idle` after; the views give state and `can_cancel` only of an agent, the state only while the
  channel runs; the security suite asks `can_cancel` as `CancelChannel`; the API names a state as
  the event does. Verified in the browser (`simple-session`, `gemma4:26b`): the agent's dot green,
  none for alice and the approver; pulsing with "Working: thinking" and a stop icon while it
  answered; its stop icon ended the answer, and the dot went back to green. AGENT-TURN.md,
  CHANNELS.md, REST-SERVER.md, WEB-UI.md, SECURITY.md.
