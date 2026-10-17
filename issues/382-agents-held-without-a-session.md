# 382 — An agent stays loaded while nothing uses it

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Epic:** [378](378-resource-management-epic.md)
**Summary:** An agent stays loaded with its memory while nothing uses it, and what still holds it once its sessions are unloaded has not been checked.

## The problem

An agent that runs in several sessions has one `AgentController` and one `AgentStateHandle` per
instance, held by the `ControllerManager` for as long as any session holds it (STORAGE.md,
*session-owned and per-instance controllers*). Its state - its memory above all - is loaded when
the first session needs it. What still holds it once its sessions are stopped or unloaded
([381](381-inactive-channels-held-in-memory.md)) has not been checked, and nothing but the
`ControllerManager`'s reference counting is meant to let it go.

## Why it matters

- An agent's memory grows with everything it remembered in every channel; an agent used once and
  then held for the server's life keeps it loaded for nothing.
- Once channels and sessions are unloaded, agents are what is left in memory; without this issue,
  381 unloads the channels and keeps their agents.

## Considerations

- **What holds an agent.** The actors of its channels, the `ChannelController`s and
  `SessionController`s that resolve its name, the views that read its context, a turn under way
  elsewhere. To be found rather than assumed; a controller that is held by something long-lived -
  a cache, a subscription of the event bus - is the bug this issue is likely to find.
- **The agent's state has one owner per process** (STORAGE.md). Unloading it means there is none
  until it is loaded again; a turn that stages changes against a snapshot and commits them under
  the handle's lock must find the handle, which loads the state again. Nothing must commit against
  a state that was unloaded in between - the handle is to be held for the turn's duration, which it
  is today.
- **Reference counting is the mechanism** already there (`Arc`, held only while used); what this
  issue adds is making sure nothing holds for longer than it uses, and a test that an agent whose
  sessions are all unloaded is gone from memory.
- **Users and workspaces** are held the same way and get the same check.

## Done when

- An agent whose channels are all unloaded or stopped is no longer held in the process, and its
  next use loads it again.
- A test shows it, for an agent in two sessions, with the second unloaded after the first.
- STORAGE.md says what holds an agent and when it is let go.

## History

- **2026-10-16** — Opened as part of the epic [378](378-resource-management-epic.md).
