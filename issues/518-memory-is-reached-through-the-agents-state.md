# 518 — An agent's memory is reached through its state, not as a system service

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Context and knowledge
**Summary:** An agent's memory is part of its state, which the agent side is handed whole and changes through three calls of its `ActorRuntime`, and the agent side itself decides which entries may be recalled where, while the workspace, skills and artifacts are system services reached through a reader and a writer.

## The problem

Issue [516](516-the-runtime-trusts-the-agents-linked-into-it.md) made the kernel a modular one:
its own services are sessions, channels, actors and their state, and persistence; the workspace,
skills and artifacts are **system services** that extend it - a manager that decides and keeps,
views for the frontends, and for an agent's work a reader and a writer the kernel binds to every
piece of work (`Situation::reader`, `Turn::writer`, the pattern of *Step 5b* there). Memory was
named among them (decision 10 there), but it is not one yet:

- The memory lies in the agent's state (`AgentState::memory`), which the kernel keeps in an
  `AgentStateHandle` and the agent side reads whole (`ActorRuntime::agent_state`).
- The agent side changes it through three calls that are the memory's alone:
  `reserve_memory_id`, and `apply_agent_state(changes, after)` when a turn ends.
- Which entries may be recalled in a channel the agent side decides itself: its memory
  (`caelumora-agents/src/memory.rs`) asks the kernel's flow question for every entry
  (`ActorRuntime::may_copy_information`), and the component `agent_memory` writes the prompt part
  of what it found.

ARCHITECTURE.md says so: "memory is still reached through the agent's state".

## Why it matters

What an agent may recall where is a decision of the kernel's, as what a work may read of another
channel is (issue [131](131-agent-memory-bypasses-policy.md) made memory ask the policy at all).
Asked per entry by the agent side, it is kept by the agent side's loop: a library that leaves the
question out recalls what it may not. As a system service, the kernel hands the work only the
entries it may recall, as `SkillReader::usable_skills` hands only the skills a work may load.

It also slims the interface: `ActorRuntime` loses `reserve_memory_id` and `apply_agent_state`,
which serve one kind of state, and the agent's state the agent side is handed no longer carries
what it may not see in a channel.

## Considerations

- **The pattern of 516.** A `MemoryReader` - the entries the work may recall where it reads from -
  handed out by the `Situation`, and a `MemoryWriter` - remembering and forgetting, staged in the
  turn and applied when it ends - handed out by the `Turn`, both bound by the kernel to every piece
  of work; a manager of memory that decides and keeps, asking the `ConfidentialityPolicy`; views
  for the frontends (the web UI shows an agent's memory, issue
  [303](303-note-and-memory-presentations.md)).
- **The kernel's, or a plugin's.** Memory is part of an agent's state, which the kernel keeps
  (`AgentStates`); the service can be the kernel's own extension, as the workspace's is, or a
  plugin's, as skills are. The kernel's is the smaller step, a plugin the cut of 516 at its
  cleanest.
- **Staging.** A turn stages its changes and applies them when it completes (ARCHITECTURE.md,
  *Stage, then commit*); the writer keeps that: what it is told is applied at the turn's end, in
  the turn's name, or not at all.
- **Topics.** Issue [166](166-memory-as-topic-documents.md) wants memory as documents by topic,
  fetched as they are needed. The reader and the writer are the interface that change would be made
  behind; doing this first keeps 166 within the kernel's service.

## Done when

- An agent's work reaches its memory through a reader and a writer the kernel binds, and the
  reader answers only the entries the work may recall where it reads from.
- `ActorRuntime` has no call of the memory's own (`reserve_memory_id`, `apply_agent_state`), and
  the agent side asks no flow question of an entry of its memory.
- ARCHITECTURE.md names memory among the system services without a reservation.

## History

- **2026-11-01** — Opened from issue [516](516-the-runtime-trusts-the-agents-linked-into-it.md),
  whose decision 10 names memory among the extensions of the kernel and whose steps left it reached
  through the agent's state.
