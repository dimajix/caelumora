# 591 — An external agent cannot be a sub-agent

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** The sub-agents of issue 558 are agents of internal personalities alone: an external agent's intake does not pass on what it is given to read, and its delegate would take a task that outlives its request for one that ended without an answer.

## The problem

Issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md) lets an agent hand a task to a sub-agent
in a thread of its own (issue [557](557-a-conversation-cannot-go-on-two-ways-at-once.md)): the
kernel's delegate gives the sub-agent the brief to read (`GivenToRead`), asks it, and gives back
what it answers. An external agent ([179](179-external-agents-via-a2a.md)) takes part in a thread
as any agent does, its remote conversation being that thread's, but as a sub-agent two things are
missing:

- **What it is given to read.** Its intake has to take in `GivenToRead` and pass it to its remote
  agent as text.
- **An answer that comes later.** A remote task can outlive its request and is answered when the
  external agent speaks on its own (issue [199](199-external-agent-speak-wakes-all.md)). The
  delegate would take the end of the turn before it as an answer that ended without one.

So the first version of 558 starts agents of internal personalities alone; the sub-agent tool
refuses an external one with an error.

## Considerations

- **Taking in what is given to read**: as text, in the request to the remote agent that follows,
  or as a message of its own to the remote conversation.
- **The delegate and a late answer**: the delegate keeps the brief open while the sub-agent has a
  remote task under way, and takes what the external agent says on its own as the answer to it; or
  the external agent's turn ends only once its task did.

## Done when

- An external agent can be started as a sub-agent: it is given the brief, and its answer reaches
  the main agent, also when its remote task ends after the turn that started it; covered by tests.

## History

- **2026-11-17** — Opened from issue 558, as the maintainer decided: external agents as
  sub-agents are a step of their own, after the first version.
