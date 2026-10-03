# 213 — An external agent's task taken up again keeps the work of the call that began it

**Status:** open
**Area:** Integrations

## The problem

An `OpenTask` keeps the `Work` it is done as ([211](211-turn-and-agent-work-structure.md),
[212](212-external-task-during-reclassification.md)): the envelope of the request, whose authority
it acts with, the channel's classification it is bound by, what it has seen and - since
[205](205-external-agent-message-context.md) - what the remote agent was given in its call
(`Work::context`). What a task says unasked later (`ExternalAgentActor::speak`) is said as that
work, and it is followed (`follow_open_tasks`) and cancelled with its authority.

A task that asked back (`input-required`) is taken up again by the next request: that call
`answers` it, and from then on it should be done as the work of *that* call. But
`RemoteState::begin`, which records that, is only called in `exchange` on `Step::Started` - when
the remote agent tells the task as begun. Whether it does is up to the remote side and the
protocol's translation: the A2A agent tells it once per call when a response carries the `Task`
object (`Translation::task`), but not for `statusUpdate` or `artifactUpdate` events, which carry
only the task's id. A remote agent that answers the continuation with updates alone leaves the
`OpenTask` with the work of the call that began it.

If that answer then breaks off before the task ends, the task is followed and what comes of it is
said unasked - as the old work:

- in the envelope and span of the first request, not of the one that answered the question;
- with the authority of the first request's sender, who may be someone else;
- bound by the channel's classification when the first request came, and labelled as what the
  first exchange had seen;
- with the context of the first call, not of the call that answered the question.

## Why it matters

Whose authority an agent acts with, and what its answer is classified as, should not depend on
whether a remote side repeats an event it need not. It is the same rule a turn follows: one request,
one piece of work.

## Considerations

1. **The call takes the task up, not the event.** In `exchange`, when the call `answers` a task,
   `state.remote.begin(&answers, &work)` right after the send, before anything comes back. What
   the remote agent tells afterwards changes nothing; `Step::Started` then only records a task the
   call *began*. Small, and independent of the protocol.
2. **The translation tells the task for every update.** `Translation` reads the task id of
   `statusUpdate` and `artifactUpdate` and tells it as `Started` on the first one. Fixes A2A only,
   and every other `ExternalAgent` would have to do the same - the contract of `ExternalAgent`
   would say so.

Option 1 puts the rule where the work is made; option 2 could be done as well, so that a task the
remote agent begins without a `Task` object is known too - that is a separate gap of the A2A agent,
to be checked against the protocol.

## Done when

- A task taken up again is done as the work of the call that took it up, whatever the remote agent
  tells, and a test shows it for a continuation answered with updates alone that breaks off.

## History

- **2026-10-03** — Issue created, noticed while doing [205](205-external-agent-message-context.md).
