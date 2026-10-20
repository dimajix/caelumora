# 439 — An agent is not told why it was asked to respond

**Status:** done
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

### The agent is told it - when it was chosen

A prompt part, in the turn, tells an agent chosen by the policy that nobody mentioned it, that it
was asked because what was said last might concern it, and that if it has nothing to add - nothing
new, only agreement, a greeting, that it waits - it calls `silence` and says nothing. It belongs to
the turn, not to the system prompt, since it changes with every turn (see
[AGENT-TURN.md](../docs/AGENT-TURN.md#instructions-within-a-turn)).

An agent that was mentioned, or nudged, is told nothing. The experiments in
[work/quiet_agents](../work/quiet_agents/RESULTS.md) (gemma4:26b, contexts of the `team-session`)
found:

*   **For the chosen agent the part works.** Where it had nothing to add, it stayed silent in 27 of
    27 answers, against 1 to 3 of 27 without the part. Where something was clearly its own - the
    coder's report of a task done, with every mention taken out - the reviewer looked for the code
    and the planner answered (6 of 6). Answers to questions and work were not touched, since the
    part is given only where nobody wanted the agent's answer.
*   **For the mentioned agent no wording was safe.** Told to stay silent when the message that
    mentioned it wants nothing of it, it silenced the small talk among agents (15 of 15) - and the
    coder, whose go had come before and who was mentioned only by a cheer, stopped working (0 of 6).
    Naming the work as well ("and go on with any work that is yours to do") brought it to 4 of 6,
    not to 6. A mentioned agent judges the message that mentioned it, not what is its own to do.
    Small talk between mentioned agents is left to the routing.
*   **Neither the roster's sentence of [438](438-agents-mention-without-knowing-it-asks.md) nor a
    cost of mentions or messages in credits, stated in the system prompt, changed anything.**

When an agent takes several requests into one turn (`AgentBody::take_in`), the last begins it and
the earlier ones are dropped. The reason should not be: a mention among them is a reason to answer,
even if the last request was only a choice. So the turn keeps the strongest reason of those taken
in - mentioned before chosen - while it acts in the contexts of the last, as today.

### Open

*   **External agents.** Their remote agent is not given prompt parts but messages; whether it is
    told why, and how, is to be decided with the A2A mapping.
*   **What `silence` is called** - decided: it stays `silence`. Renamed `pass`, the tool was called
    less reliably and once written out as text instead.

## Done when

*   A request to respond carries why it was made, recorded with it in the history.
*   An agent in a turn that was chosen by the policy, and not mentioned or nudged, is told so, and
    that with nothing to add it stays silent.
*   [CHANNELS.md](../docs/CHANNELS.md#moderators) and [AGENT-TURN.md](../docs/AGENT-TURN.md)
    describe both.

## History

- **2026-10-20** — Opened after a look at the `team-session` of the seed, where agents chosen by
  the decision model answered with small talk or wrote that they had nothing to say.
- **2026-10-20** — Experiments on the `team-session`'s contexts (`work/quiet_agents`): telling the
  chosen agent why it was asked silences it where it had nothing to add; telling a mentioned agent
  when to stay silent stops it working, too; a chosen agent with something of its own to do still
  does it. The part is narrowed to the chosen agent; the tool stays `silence`.
- **2026-10-20** — Done: a `Routing` says why it asks (`RequestReason`: `mentioned`, `nudged`,
  `chosen`), the request to respond carries it and the history records it; the work of a turn
  keeps the strongest reason of the requests it took in (`Work::asked`). The part is a component,
  `request_reason_prompt_provider`, before the turn's input - placed there, it worked as well as
  after everything heard (`reason_before`) -, with `silence` where it is offered; the seed's
  `channel-context` includes it. An external agent's work records the reason too, but its
  personality in the seed does not include the provider; what its remote agent is told stays with
  the A2A mapping.
