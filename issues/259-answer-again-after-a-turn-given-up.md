# 259 — An agent cannot be asked to answer again after a turn it gave up

**Status:** open
**Area:** Sessions and channels

## The problem

Since [258](258-what-was-said-is-lost.md), a turn given up keeps what it said, followed by its end.
Asking for another answer then has nothing to aim at: the retry of a message refuses a message
followed by a turn's end as `NotLast` (`AgentChannelState::retried`), and the retry of a turn
(`RetryTarget::Turn`) only reaches a turn still under way - one that ended is refused as
`NotUnderWay`. After a turn failed - the model unreachable for a moment, say - the user can only
write something again, or wake the agent.

## Why it matters

A failed turn is the case where answering again is wanted most, and the web UI offers no icon for
it: the notice of the turn given up has none, and the message before it is not the agent's last
answer any more.

## Considerations

- **A turn given up is asked for again as a turn:** `RetryTarget::Turn` of a turn that ended given
  up is accepted as a request to answer anew after it, taking nothing back - its messages stay, as
  258 decided. Its answer is then a turn of its own, in the contexts of the request.
- **Or the notice offers a wakeup** of the agent, in the request's name: no new concept, but the
  agent does not know it is to answer the same question.
- What the model is given is decided already: the turn given up and why stand in its context.

## Done when

- A user who may ask an agent to answer again can do so after a turn the agent gave up, and the web
  UI offers it on the turn's notice.

## History

- **2026-10-08** — Opened when closing [258](258-what-was-said-is-lost.md), from part J of its plan.
