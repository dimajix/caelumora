# 175 — No moderator lets agents act only when a person writes

**Status:** open
**Area:** Sessions and channels

## The problem

In a channel with several agents, every moderator but `mentions_only` asks
agents to respond to the messages of *other agents*, not only to those of
users. `PolicyModerator::route_full_response` applies the policy to every
complete response, whoever sent it; only the sender itself is left out of
the candidates (`ModeratorActors::actor_candidates`).

- Under `broadcast`, an agent's answer makes every other agent answer, and
  each of their answers does the same.
- Under `round_robin` and `random`, one agent answers the next.

That is intended: these moderators are for channels in which the agents
discuss with each other. But there is no moderator for the other case, which
many multi-agent channels want: a person writes, every agent answers, and
then the agents are quiet until a person writes again. `mentions_only` is
quiet, but then no agent answers a user who mentions nobody.

## What is wanted

A new moderator (working name `user_driven`):

- **A user's message** goes to every agent, and every agent that is awake is
  asked to respond - like `broadcast`. If the user mentions agents, only
  those respond, as under every moderator.
- **An agent's message** is delivered to the other agents like under every
  moderator, but asks none of them to respond.
- **Exception:** an agent that `@`-mentions another agent asks that agent to
  respond - which is [45](45-agent-mentions.md).

Agents thus become active only when a person writes, or when an agent
explicitly addresses another.

## Considerations

### Delivered, but nobody is asked to respond

**Decided:** an agent's message is delivered to the other agents, into their
transcripts, as under every moderator; it only starts no turn. Loops come
from asking to respond, not from delivering. Not delivering would let the
agents' transcripts drift from the channel's history (the principle behind
[76](76-channel-cancel-flag.md) and [90](90-message-path-wake-and-queue.md)),
make the agents repeat or contradict each other unknowingly, and leave an
agent that is mentioned later without knowing what it responds to.

### Mentions between agents are 45

The exception is what [45](45-agent-mentions.md) builds for every
moderator: a mention written by an agent makes the mentioned agent respond,
bounded by a limit on consecutive agent-to-agent hops since the last user
message. This moderator adds no mention rule of its own. Until 45 is done,
it is simply `broadcast` for users' messages and nobody for agents'; with 45
the exception comes by itself.

### What the `ResponderPolicy` has to know

Mentions stay in `PolicyModerator` (`mentioned_responders`, and the hop
limit of 45). What remains for this moderator is "ask every candidate if a
user wrote, nobody if an agent did" - a policy, but one that needs to know
who wrote. `ResponderPolicy::choose` receives only the candidates today.

- **(a) Give the policy the sender** (or the message's `ActorId` kind), e.g.
  `choose(sender, candidates)`. The existing policies ignore it; the new one
  is a few lines. The policy stays the one thing a moderator varies in.
- **(b) A parameter of `PolicyModerator`** ("only users' messages ask
  anybody") that combines with any policy. More general - `round_robin`
  among the agents, but only for users' messages - but a second axis next to
  the policy, and the four existing moderators would each have to say what
  they are on it.

Recommended: (a). It keeps one extension point, and it is the information a
policy component would need in [174](174-moderator-as-component.md) as well -
a policy as a component should not be blind to who wrote. Whether the policy
needs more than the sender (the message itself, for a policy that decides by
content) is a question for 174.

### Compatibility

The web UI explains the moderators it knows (`caelumora-web-ui/src/moderator.ts`)
and offers them when a channel is created; the new one needs a label and an
explanation there. Other kinds are shown by their name, so nothing breaks
before.

## Done when

- A moderator exists under which a user's message makes every awake agent
  respond (the mentioned ones, if any), and an agent's message is delivered
  to the other agents but makes none of them respond.
- With [45](45-agent-mentions.md), an agent's mention makes the mentioned
  agent respond under it too.
- The existing moderators behave as before.
- The web UI explains and offers the new moderator.
- [CHANNELS.md](../docs/CHANNELS.md#moderators) describes it, including that
  `broadcast`, `round_robin` and `random` let agents answer each other.

## History

- **2026-10-01** — Opened.
- **2026-10-01** — Decided with the maintainer: agents' messages are
  delivered but ask nobody to respond; the existing moderators letting
  agents answer each other is intended, hence a new moderator; mentions
  between agents and their bound are [45](45-agent-mentions.md), not part of
  this issue; the `ResponderPolicy` needs to know who wrote.
