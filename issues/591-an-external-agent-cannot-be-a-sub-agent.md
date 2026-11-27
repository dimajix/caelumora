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

### What happens today with a late answer

Read in the code on 2026-11-27, for the second point of *The problem*:

- A request whose remote task outlives it - the stream ends or breaks before the task does - is
  answered with nothing: `Ending::Answered(None)`, published as a `TurnFinished` without messages
  (`caelumora-agents/src/actor/external_agent.rs`, `exchange`, `answered`). What came so far is
  kept with the task, to be said with the rest when it ends (issue 195).
- The delegate takes the recipient's first turn that ends after the brief was given as its answer
  (`DelegateActor::take_in`), so it gives back "ended without an answer"
  (`kernel.delegation_unanswered`).
- The main agent's word on it is then due (issue 598): a turn of the giver that ends without a
  brief lets the delegation go, the session follower closes the thread, and the external agent
  leaves the channel with it - and leaving cancels every open remote task
  (`ExternalAgentActor::run`, `has_left`). **The late answer is lost, and the remote work is
  cancelled.**
- Inferred, not tried: the task's later end is said with the turn id of the request that began it
  (`speak`, `answered`), as a `TurnFinished` after the brief was given. Had the main agent given a
  next brief meanwhile, the delegate would take the late answer as that brief's.
- A question back from the remote agent already works: it is the request's answer, given back as
  such, and `ask_subagent` continues the task with the next request.

Not looked at: how the intake takes in `GivenToRead` (the first point of *The problem*).

### Approaches to a late answer

- **A. The request ends only with its task.** The external agent does not answer a request whose
  task is still open; its turn lasts until the task ends, and what it would have said on its own is
  the request's answer. The delegate needs no change. But a cancel or a stop has to reach a turn
  that may last hours, the channel shows the agent busy all that time, and while it answers a
  request what its other tasks send waits (CHANNELS.md *External agents*) - for hours too.
- **B. The delegate knows a late answer.** The end of a request whose task is still open is marked
  as such - a reason on `TurnFinished`, or an event of its own. The delegate then gives back that
  the sub-agent is still working and its answer will follow, keeps the brief open, takes the task's
  later end as its answer, and the giver's word is not due before. The main agent is told what
  happens; it needs the mark on the agent side and a state of the brief in the delegation.

Recommended: B. An external agent's work is long by nature, and the main agent should be told that
it is under way rather than be given an empty answer or wait in silence.

## Done when

- An external agent can be started as a sub-agent: it is given the brief, and its answer reaches
  the main agent, also when its remote task ends after the turn that started it; covered by tests.

## History

- **2026-11-17** — Opened from issue 558, as the maintainer decided: external agents as
  sub-agents are a step of their own, after the first version.
- **2026-11-27** — What happens today with a late answer read in the code, and two approaches to
  it added, B recommended (*What happens today with a late answer*, *Approaches to a late answer*).
