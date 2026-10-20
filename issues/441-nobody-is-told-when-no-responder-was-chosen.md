# 441 — Nobody is told when a responder policy could not choose who answers

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** When the decision model of `decision_policy` does not choose - a timeout, an error, an answer naming no option - and nobody is asked, a person's message that mentions nobody stays unanswered without anyone telling them why.

## The problem

With [437](437-mentions-of-people-and-failed-decisions-ask-agents.md), a `decision_policy` whose
model does not choose in time asks nobody by default (`on_failure: none`) rather than every agent.
That keeps a slow or failing model from making all agents answer, but a person's question that
mentions nobody then goes unanswered, and nothing tells them why or what to do: the channel is
just silent. For a chain cut off at `max_agent_hops` the users are told
([200](200-chain-limit-hint.md), `Notice::ChainCutOff`); for a decision that failed they are not.

## Considerations

*   **A notice of the moderator.** What the moderator tells of its own is a fixed set of notices
    (`Notice`, issue 418), told in the system's name to whom it names. A notice that it could not be
    decided who answers - "mention someone with @ to ask them" - fits there, to the users.
*   **The policy cannot say that it failed.** `ResponderPolicy::choose` returns the ids to ask, so
    the moderator cannot tell a choice of nobody from a failure. Telling the users needs `choose` to
    answer more - for instance a failure beside the ids, or a result type - for every policy. That
    is an architecture question: what a policy answers, and whether a failure is the policy's to
    report or the moderator's to notice.
*   **Not for every failure.** Under `on_failure: all` or `random`, somebody is asked; a notice
    would only confuse. It belongs to the case in which nobody is asked because the decision
    failed.

## Done when

*   A message after which nobody is asked because the decision failed is followed by a notice to
    the users that says so and how to ask someone.
*   [CHANNELS.md](../docs/CHANNELS.md#choosing-by-what-was-said) describes it.

## History

- **2026-10-20** — Opened when [437](437-mentions-of-people-and-failed-decisions-ask-agents.md)
  made a failed decision ask nobody by default; the maintainer decided that telling the users is
  an issue of its own.
