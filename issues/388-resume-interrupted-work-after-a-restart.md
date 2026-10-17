# 388 — Work interrupted by a restart waits until someone acts in its session

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** Work interrupted by a restart - a turn stopped mid-step at the shutdown - waits until someone acts in its session, since nothing starts the session again.

## The problem

An orderly shutdown ([297](297-orderly-shutdown.md)) stops every running session as a stop does: an
agent in the middle of a turn stores the step it stands at - `Planning`, `ExecutingTools`,
`WaitingForUser` - to be resumed when its channel runs again (issue 377). But nothing starts the
channel again after the restart. Since [386](386-actor-state-after-a-stop.md) not even a browser
following it does: a session runs again only when someone acts in it - writes, approves, retries.
Until then the turn that was under way stands still, and whoever follows the channel sees its
agents as not running.

## Why it matters

A deploy is an ordinary event. An agent working through a plan of tool calls when the server is
restarted should go on with it, not wait for a user to come back and say something - who, seeing
the agent not running, may not know that it had work left.

## Considerations

- **Not every session.** Starting every session that ran at the shutdown would bring back into
  memory the idle ones as well, which [381](381-inactive-channels-held-in-memory.md) is about to
  give back. What is worth starting is a session in which work was under way: an actor that was not
  `Idle` or `Sleeping` when it was stopped.
- **Noted at the shutdown.** `SessionManager::shutdown` knows the state of every actor it stops; it
  could note the sessions with work under way in the state store, and the server start those at its
  start - and forget the note. Cheap, but a crash, the OOM killer or SIGKILL leave no note.
- **Or read at the start.** The stored states of the agents tell which turns stood mid-way, also
  after a crash; reading them all at every start costs a pass over every agent's state of every
  channel, unless the store keeps an index of the turns under way.
- **Whose name it runs in.** A session started by the server at its start is started in the
  runtime's name, as nobody asked for it; what the agents then do is in their own names, as when a
  user had started it.
- **Bounded.** A server that went down with many turns under way should not start all of them at
  once; a limit, or starting them one after another.

## Done when

- After an orderly restart, a turn that was under way when the server stopped goes on by itself.
- It is decided, and written down, whether that holds after a crash as well.

## History

- **2026-10-16** — Split from [297](297-orderly-shutdown.md) while deciding its open points with the
  maintainer: the shutdown comes first; resuming what it interrupted is a step of its own.
