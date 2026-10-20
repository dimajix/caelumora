# 439 — An agent is not told why it was asked to respond

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** A request to respond carries no reason, so an agent cannot tell whether it was mentioned or only chosen by a responder policy that guessed it might fit, takes every request as an order to answer, and answers with small talk or that it has nothing to say instead of staying silent.

## The problem

The moderator asks agents to respond for different reasons: a message mentions them, `@all`
mentions everyone, a user nudges them, or the channel's `ResponderPolicy` chooses them - every
agent, the next in turn, one at random, or those a decision model finds likely to fit
([403](403-decision-model-policy.md)). Only the first are meant for the agent; a choice of the
policy is an offer, made without knowing whether the agent has something to say.

The agent does not learn which it is. `ActorEvent::ResponseRequested` carries nothing but its
contexts, the request is not in the transcript, and the model sees the conversation and that it is
its turn. So it answers: in the `team-session` of the seed, the planner and the coder, chosen by the
decision model, greet back when Sassy greets a person; Jonathan writes "(No response needed yet, as
I am not addressed in the current conversation.)", the reviewer "Noch keine Änderungen zum Review
vorgelegt. Ich warte auf weitere Nachrichten." Both wanted to stay silent and said so instead. The
`silence` tool exists and is sometimes used, but nothing tells the model when it is the expected
answer.

Each such answer distracts the agents that work and, worse, can make others answer in turn. The
model call itself is not saved by staying silent - only by asking fewer, which is
[437](437-mentions-of-people-and-failed-decisions-ask-agents.md).

## Considerations

### The request says why

The moderator knows why it asks when it builds its `Routing`: `ask` would name, for each agent, the
reason - for instance `Mentioned { by }`, `MentionedAll`, `Nudged { by }`, `Chosen`. The channel
builds the `ResponseRequested` from it, and the history records it with the request
(`HistoryEntry::ResponseRequested`), so that what the agent was told can be read again.

`Chosen` covers every policy alike: under `broadcast_policy` or `round_robin_policy` an agent is
chosen as much as by the decision model. Whether it also carries how sure the policy was - the
probability of a decision model - is open; a number the model is told invites it to second-guess,
and the other policies have none.

### The agent is told it

A prompt part, in the turn, says why the agent was asked: when mentioned, that someone wants its
answer; when chosen, that it was asked because it might have something to add, and that if it has
not - nothing new, only agreement, a greeting, that it waits - it calls `silence` and says nothing.
It belongs to the turn, not to the system prompt, since it changes with every turn (see
[AGENT-TURN.md](../docs/AGENT-TURN.md#instructions-within-a-turn)).

When an agent takes several requests into one turn (`AgentBody::take_in`), the last begins it and
the earlier ones are dropped. The reason should not be: a mention among them is a reason to answer,
even if the last request was only a choice. So the turn keeps the strongest reason of those taken
in - mentioned before chosen - while it acts in the contexts of the last, as today.

### Open

*   **External agents.** Their remote agent is not given prompt parts but messages; whether it is
    told why, and how, is to be decided with the A2A mapping.
*   **What `silence` is called.** A model that writes that it has nothing to say rather than call a
    tool may follow better when the tool's name and description speak of passing - "nothing to
    add" - than of silence. Worth trying before anything is built around it.

## Done when

*   A request to respond carries why it was made, recorded with it in the history.
*   An agent in a turn is told why it was asked, and that when chosen and with nothing to add it
    stays silent.
*   [CHANNELS.md](../docs/CHANNELS.md#moderators) and [AGENT-TURN.md](../docs/AGENT-TURN.md)
    describe both.

## History

- **2026-10-20** — Opened after a look at the `team-session` of the seed, where agents chosen by the
  decision model answered with small talk or wrote that they had nothing to say.
