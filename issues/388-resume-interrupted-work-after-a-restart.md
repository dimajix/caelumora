# 388 — Work interrupted by a restart waits until someone acts in its session

**Status:** in progress
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

## Decision

Decided with the maintainer on 2026-10-20: **an index of the work under way, kept by the state
store**, rather than a note at the shutdown or a scan at the start.

- **What is work under way** is the model's rule, `AgentChannelState::has_work_under_way`: a turn
  stored at a step it goes on from by itself - not `WaitingForUser`, which waits for a person, who
  starts the session by answering -, or an external agent's open task the remote agent is working
  on - not one `InputRequired`, which waits as well.
- **The state store keeps the index** as it puts an agent's channel state: the pair of agent and
  channel is in it while the state has work under way, and leaves it with the put that has none,
  and with the state deleted. It is written only when that changes, not at every step.
  `StateStore::channels_with_work_under_way` lists the channels. Since a turn is stored after every
  step, the index is as current after a crash as after an orderly shutdown: work cut off by a
  crash goes on as well.
- **`SessionManager::resume_interrupted`** starts the session of every such channel once, in the
  runtime's name - as `join` opens one -, called by the server after it is seeded. A session that
  no longer exists or is deleted is passed over.
- **Started one after another**, not bounded otherwise: starting costs little, the turns that then
  run cost. A real bound, which waits for a session's work to end, is an issue of its own under
  [378](378-resource-management-epic.md).

Not covered: words delivered to an agent but not yet taken in leave it `Idle`, so a message written
just before the shutdown waits for the next act in its session. With several instances
([122](122-several-instances.md)) every instance would start what the index lists.

## Steps

1. **The model and the stores**: `has_work_under_way`, the index in `JsonFileStore` and
   `MemoryStore`, `channels_with_work_under_way`, the state store suite; STORAGE.md.
2. **Resuming**: `SessionManager::resume_interrupted`, called by `caelumora-app`; tests of a
   restart over the same stores; CHANNELS.md, AGENT-TURN.md.

## Done when

- After an orderly restart, a turn that was under way when the server stopped goes on by itself.
- It is decided, and written down, whether that holds after a crash as well.

## History

- **2026-10-16** — Split from [297](297-orderly-shutdown.md) while deciding its open points with the
  maintainer: the shutdown comes first; resuming what it interrupted is a step of its own.
- **2026-10-20** — Decided with the maintainer: an index of the work under way kept by the state
  store, started one after another in the runtime's name by `SessionManager::resume_interrupted`;
  `WaitingForUser` and an external agent's task `InputRequired` are no work under way; a real bound
  is an issue of its own. Recorded under *Decision* and *Steps*; implemented on the branch
  `issue-388-resume-after-restart`.
