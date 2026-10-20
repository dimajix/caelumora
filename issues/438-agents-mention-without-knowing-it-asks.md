# 438 — An agent does not know that mentioning another agent makes it answer

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** An agent is told to mention others by their `@handle` to address them, but not that a mention of an agent asks that agent to respond, so agents greet and thank each other with `@` and every greeting starts another turn.

## The problem

`channel_participants_prompt_provider` tells an agent who else is in its channel and how to reach
them: "The others - mention one by their @handle to address them, or all of them with @all". For a
model that reads as a form of address, as among people: it greets with `@`, thanks with `@`, and
refers to others with `@`. But a mention is an instruction to the moderator: every agent a message
mentions is asked to respond ([45](45-agent-mentions.md),
[CHANNELS.md](../docs/CHANNELS.md#-mentions)), and `@all` asks every agent.

The `team-session` of the seed shows what follows: the coder answers "Hallo @sassy! …", Sassy is
asked and answers; the planner writes "@sassy, keine Sorge … Und @coder, …", both are asked; the
coder replies "Alles klar, @planner! …". Every one of these turns is a model call over the whole
context, distracts the agents that work, and ends only at `max_agent_hops`.

## Considerations

*   **Tell the agent what a mention does.** The roster is where the agent learns about `@` (issue
    [176](176-participants-in-system-prompt.md), [401](401-mention-everyone.md)), so the
    consequence belongs there, next to how to write a mention: that `@handle` of an agent and `@all`
    ask them to respond and cost a turn, and that one who wants no answer writes the name without
    `@`. A mention of a person asks nobody - it tells them they were meant
    ([404](404-unread-and-mentions-in-overview.md)) - and stays as it is.
*   **Only where there is someone to ask.** The sentence is about other agents; in a channel where
    the agent is the only one, it would be noise. The roster knows whether the others include an
    agent (`ActorDescription::is_person`).
*   **External agents.** The roster is given to an external agent's remote agent as well
    ([204](204-external-agent-roster.md)), whose mentions are routed alike; the same words fit.
*   **Not a rule of the routing.** That the moderator could ignore mentions in greetings would need
    it to read the text; what a mention means stays the channel's rule. Fewer asks from the
    routing itself are [437](437-mentions-of-people-and-failed-decisions-ask-agents.md).

## Done when

*   The roster says, where the others include an agent, that mentioning an agent or `@all` asks
    them to respond, and that a name without `@` refers to someone without asking them.
*   [AGENT-TURN.md](../docs/AGENT-TURN.md#who-is-in-the-channel) says so.

## History

- **2026-10-20** — Opened after a look at the `team-session` of the seed, where the agents'
  greetings to each other, each with a mention, kept asking the next one to answer.
- **2026-10-20** — Done: the roster says, where the others include an agent, that mentioning an
  agent or `@all` asks it to respond in a turn of its own, and that a name without `@` refers to it
  without asking. A person's mention is not mentioned: it asks nobody.
