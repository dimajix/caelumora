# 437 — A message to a person makes agents answer, and a failed decision asks every agent

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** A message that `@`-mentions only a person is answered by the agents the responder policy chooses, and when the decision model of `decision_policy` does not choose in time, every agent of the channel is asked - so in a channel of several agents, agents answer what was not meant for them.

## The problem

In a channel with several agents, agents that have nothing to contribute but are asked to respond
answer anyway, with greetings, acknowledgements and notes that they are waiting. That distracts the
agents that work and costs a model call per agent asked - the full context, even for an agent that
then stays silent. The `team-session` of the seed shows it: Sassy writes "Hallo @alice! …", the
`decision_policy` chooses the planner and the coder, both greet back, and their greetings - which
mention Sassy - start the next round, until `max_agent_hops` cuts the chain off.

Two rules of the routing ask more agents than the message meant:

*   **A mention of a person does not count as addressing.** The `policy_moderator` asks the agents a
    message mentions instead of its policy (`PolicyModerator::ask`, `mentioned_responders`), but
    only agents count: a message that mentions only users falls back to the policy, as the test
    `mentions_of_non_agents_fall_back_to_the_moderator_policy` fixes. A message to `@alice` is
    answered by whichever agents the policy chooses - under `broadcast_policy` by all of them.
*   **A failed decision asks everybody.** When the decision model does not choose - a timeout, 5 s
    unless configured, an error, an answer naming no option -, `decision_policy` asks whom
    `on_failure` says, `all` by default (see
    [CHANNELS.md](../docs/CHANNELS.md#choosing-by-what-was-said)). A slow model thus makes every
    agent answer, the opposite of what the policy is there for.

## Considerations

### A mention of anyone replaces the policy

That a mention replaces the policy is a rule of the moderator, not of a policy: it holds for every
policy alike. So whether a mention of a person counts is the moderator's to configure, a parameter
of `policy_moderator` - for instance `mentions_replace_policy: agents | anyone`, the name to be
settled:

*   `agents` - as today: only a mention of an agent replaces the policy.
*   `anyone` - a message that mentions any participant is addressed to them: the mentioned agents
    are asked, no one else, and the policy is not asked. A message to `@alice` alone asks nobody.

What a mention of oneself or of an author of the message means stays as it is: it asks nobody. A
chain cut off at `max_agent_hops` is unaffected.

**To decide:** the default. `anyone` is what a message to `@bob` almost always means, and would end
the round in the `team-session`; it changes how existing channels route. `agents` keeps them as
they are and leaves `anyone` to the template.

**Decided:** `anyone`, the parameter named `mentions_replace_policy`.

### Whom a failed decision asks

The decision model is asked only when the message mentions no agent - with `anyone`, no participant
at all. Asking "only the mentioned" when it fails therefore asks nobody: it is `on_failure: none`,
which `decision_policy` already offers. What changes is the default, from `all` to `none`, so that
a model that is slow or down leaves the agents quiet instead of making all of them answer.

`all` was the default so that nothing said is left unanswered in silence. With `none`, a person's
question that mentions nobody goes unanswered when the model fails, and nobody tells them why. A
notice to the users - "it could not be decided who answers; mention someone with @" - would close
that gap, as `ChainCutOff` does for a chain cut off ([200](200-chain-limit-hint.md)). It needs more
than a default: `ResponderPolicy::choose` returns the ids to ask, so a policy cannot tell the
moderator that it failed rather than chose nobody, and the notices are the moderator's (`Notice`).
Telling the users would widen what `choose` answers for every policy - an architecture question of
its own.

**To decide:** whether the notice is part of this issue, or `none` comes first and the notice
separately.

**Decided:** `none` comes first; the notice is
[441](441-nobody-is-told-when-no-responder-was-chosen.md).

### Not part of this issue

*   A separate `max_responders` for messages of agents: `max_responders` (default 1) already bounds
    them; the `team-session` sets it to 2.
*   That an agent is told why it was asked - mentioned, or chosen by a policy - so that it may stay
    silent when it was only chosen, and that agents use `@` only when they want an answer: both are
    about the agent, not the routing.

## Done when

*   `policy_moderator` takes the parameter, with the default decided, and a message that mentions
    only people asks nobody under `anyone` - for every policy, in the moderator's tests.
*   `decision_policy` asks nobody by default when its model does not choose; `all` stays
    configurable.
- **2026-10-20** — Decided: `mentions_replace_policy` defaults to `anyone`, and the notice of a
  failed decision is an issue of its own, 441.
- **2026-10-20** — Done: `policy_moderator` takes `mentions_replace_policy` (`anyone`, `agents`); a
  message that mentions only people asks nobody under `anyone`, and a mention of its own sender
  counts for nothing. `decision_policy` asks nobody by default when its model does not choose. The
  seed's `team-session` names both.
*   [CHANNELS.md](../docs/CHANNELS.md#moderators) describes both, and the seed's `team-session`
    says what it uses.

## History

- **2026-10-20** — Opened after a look at the `team-session` of the seed, where agents not meant
  answered a greeting to a person and to each other. The maintainer asked for both rules to be
  configurable.
