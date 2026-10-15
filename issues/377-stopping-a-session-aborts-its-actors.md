# 377 — Stopping or deleting a session aborts its actors

**Status:** open
**Area:** Sessions and channels

## The problem

`SessionManager::stop_running`, which both `SessionManager::stop` and `SessionManager::delete` call,
aborts the session's task, and with it every channel and every actor at once. No actor is told, none
is given time:

*   an agent's turn ends mid-step. It resumes from its last stored step when the session runs again,
    as after a crash ([75](75-agent-resumes-half-turn.md)); a tool under way is aborted rather than
    interrupted ([161](161-tools-cannot-be-interrupted.md)) and is answered as not executed, though
    it may have had its effect;
*   deleting a session ends its actors differently from deleting one of its channels: there,
    `SessionLoop::delete_channel` stops the actors with `stop_actors` and waits `STOP_GRACE` for
    them ([187](187-deleted-channel-keeps-remote-tasks.md)) - the agents give their turn up and an
    external agent cancels its open remote tasks. A deleted session leaves them running.

Behind it, the actor knows only one way to end: `Actor::stop`, which means leaving the channel for
good - a turn under way is given up, not kept for a resume (issues 75, 145). A channel that merely
stops has no way to tell its actors, though one of them already tells the two apart:
`ExternalAgentActor::run` cancels its remote tasks only when it left - "a channel that merely stops
keeps them, to be followed again" - and reaches the second case only through the abort.

## Why it matters

A session stopped by hand, and soon every session at a shutdown
([297](297-orderly-shutdown.md)), should leave its actors as they would leave themselves: each turn
stored at a step boundary to be resumed, a tool interrupted and recorded rather than cut off, the
remote tasks kept. And a deleted session should end its actors as a deleted channel does. The name
`stop` for leaving makes the missing case easy to overlook.

## Considerations

- **Two ends of an actor.** *Stopping* (the session is stopped, the process shuts down): the task
  ends, a turn under way is stored at the next step boundary and resumed when the channel runs
  again, remote tasks are kept. *Leaving* (removed from the channel, the channel or the session
  deleted): as `stop` does today.
- **Two methods, not a parameter** - `Actor::stop` and `Actor::leave`, discussed with the
  maintainer: the names say what happens, and no new type is needed. The mechanism stays one:
  synchronous like `cancel`, every actor ends its own task, the channel waits `STOP_GRACE` and
  aborts what has not ended. In the agents, `RunControl` keeps two kinds of interrupt, decided at
  the one place after a step (`AgentActor::run`). No default implementation for either, for the
  reason of 187: an actor that forgets one must not compile silently.
- **The session stops its channels.** A `SessionLoop::stop` stops the actors of every channel, in
  parallel, and waits for them; `stop_running` calls it before it aborts the task, with stopping for
  `stop` and leaving for `delete`. `ChannelLoop::stop_actors` takes the same distinction. A stop by
  hand then takes up to `STOP_GRACE` instead of returning at once.
- **What a stopped agent stores.** The turn's step as it is now, so that the resume rules of 75
  apply unchanged. A tool interrupted by the stop needs a result that says so, which the model is
  given when the turn resumes - how exactly is to be decided with the implementation. An approval
  the agent waits for stays pending.
- **A state `Stopped`?** Raised by the maintainer. `ActorState` is both what is announced and the
  stored `processing_state` a resume starts from; a stopped agent has to keep `Planning` or
  `ExecutingTools` stored, so a stored `Stopped` would push out what the resume needs. Whether an
  actor runs is its channel's to say (`channel::STOPPED`); if observers are to stop seeing the last
  state of an actor that no longer runs, the views could report none for a channel that is not
  running. Open.

## Done when

- `Actor` has `stop` and `leave`; removing a participant and deleting a channel or a session leave,
  stopping a session stops - each with the grace of `STOP_GRACE`.
- An agent stopped mid-turn resumes the turn when its session runs again, an interrupted tool
  recorded as such; an external agent keeps its remote tasks when stopped and cancels them when its
  session is deleted - covered by tests, in the security suite where an action in someone's name
  changes.

## History

- **2026-10-15** — Opened while looking at [297](297-orderly-shutdown.md) with the maintainer: 297
  assumed that stopping a session gives its actors the time to store their state, which it does
  not. An orderly shutdown needs this first.
