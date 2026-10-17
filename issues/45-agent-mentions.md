# 45 — Mentions written by agents are not routed

**Status:** done
**Area:** User interaction
**Summary:** An `@`-mention written by an agent does not make the mentioned agent respond, so agents cannot hand questions to each other.

## The problem

An `@`-mention written by a user makes the mentioned agents respond ([29](29-mentions.md)). A
mention written by an agent does nothing: it is resolved and highlighted, but
`ModeratorActors::mentioned_responders` returns `None` for any sender that is not a user. An agent
cannot hand a question to another agent (`@researcher, can you look this up?`). This holds for every
moderator, including `mentions_only` ([44](44-mentions-only-moderator.md)), where agents therefore
never answer each other at all.

Mentions by agents were left out on purpose. Two agents addressing each other would talk
indefinitely, and nothing bounds such a chain today in a way that reflects what happened.

## Why it matters

- Delegation between agents is the reason to put several specialised agents into one channel.
  Without it the user has to relay every question by hand.
- `mentions_only` makes the gap obvious: it is the moderator where addressing is the only way to get
  an answer.

## Considerations

### What has to be tracked

To bound a chain, the channel must know, for an agent's message, how many agent-to-agent hops lie
between it and the last message a user wrote. A user message starts a new chain; every agent answer
that was triggered by an agent mention extends it.

### What exists: `span_id` and `span_depth`

`EventContext` already carries `span_id` and `span_depth` (`caelumora-model/src/actor.rs`).
`EventContext::next` keeps the span and increments the depth, and `ChannelLoop::route_event_impl`
sets both on every forwarded envelope and stops routing at `max_route_decisions` (a hard-coded 100),
with only a `warn!`.

That is the right idea, but it measures something else:

- `span_depth` counts *routing steps*: forwards, `ResponseRequested`, the events of a turn. One user
  message followed by one answer already consumes several steps, so the depth says little about how
  many agents took part.
- The limit is a safety net against runaway routing, not a rule users can understand or configure,
  and hitting it is silent.

Options:

1. **Count agent hops in the span.** Add a field, e.g. `agent_hops`, to `EventContext`. It is reset
   when a user message starts a span and incremented when an agent's mention makes another agent
   respond. The moderator routes an agent mention only while it is below a limit. This reuses the
   span that already ties a chain together, and the count travels with the envelope.
2. **Derive it from `span_depth`.** No new field, but the limit would depend on how many events a
   turn produces, which changes whenever the agent loop changes.
3. **Track it in the moderator.** The moderator remembers per span how many agent hops it has
   routed. That needs no change to `EventContext`, but it is state that is lost on restart and has
   to be cleaned up.

Option 1 looks cleanest. It also needs the span to begin at the user's message and to survive the
agent's turn up to its `FullResponseGenerated`, which has to be verified.

### The limit

- **Where it is configured:** per channel in the template, next to the moderator, with a default
  (e.g. 3), or globally.
- **When it is reached:** the mention is not routed. Whether users are told (a hint, like a notice)
  or it happens silently has to be decided; silently is what `mentions_only` does for a message
  without a mention.
- **What it replaces:** whether `max_route_decisions` stays as a separate safety net or is replaced.

### What agents need to know

Agents only use mentions if they know the notation and the channel's handles. That fits a
`SystemPromptProvider` contribution listing them. An agent also needs to see in its context that a
message was addressed to it, and by whom.

### Self-mentions and mentions of the sender

An agent mentioning itself, or the agent that addressed it, must not create a loop that the limit
alone stops. `mentioned_responders` already skips the senders of the messages; whether that is
enough across hops has to be checked.

## Done when

- A mention written by an agent makes the mentioned agents respond, under every moderator, up to a
  limit on consecutive agent-to-agent hops since the last user message.
- It is decided and documented how the hops are counted, where the limit is configured and what
  happens when it is reached.
- Agents learn the channel's handles through their system prompt.
- Tests cover a chain that stops at the limit and a user message that starts a new chain.

## History

- **2026-09-25** — Issue created, split off from [44](44-mentions-only-moderator.md). The idea to
  track the chain through the span (`span_id` plus a hop count) comes from the discussion there.
- **2026-10-01** — [175](175-user-driven-moderator.md) asks for a moderator under which agents'
  messages make nobody respond; its only exception, an agent's mention, is this issue.
- **2026-10-02** — Discussed and decided; not yet implemented. What was found first:
  - The handles no longer need a contribution of their own: `channel_members_prompt_provider`
    ([176](176-participants-in-system-prompt.md)) already names every member with its handle.
  - The span survives the turn: an agent's turn builds its context with `context.next` from the
    `ResponseRequested` (`AgentActor`, and `ExternalAgentActor::respond`).
  - Mentions are not the only chain between agents. Without a mention, `PolicyModerator` asks its
    policy for an agent's message as for a user's, so under `round_robin_policy` (the default),
    `random_policy` and `broadcast_policy` agents already answer agents today, bounded only by
    `max_route_decisions`.

  Decided:
  - **What is counted:** every agent-to-agent hop, not only those caused by a mention. A field
    `agent_hops` in `EventContext`: `EventContext::new` starts at 0, so a user's message starts a
    new chain without a reset of its own; `next` keeps it; the moderator increments it on a
    `ResponseRequested` it routes for an agent's message. `route_event_impl` leaves it as it is.
  - **Where the limit is checked:** in `PolicyModerator::route_full_response`, once for the
    mentioned agents and the policy's choice alike. `mentioned_responders` drops its check for a
    user sender.
  - **Where it is configured:** as a parameter of the moderator component (e.g. `max_agent_hops`,
    default 3), not globally.
  - **When it is reached:** the hop is not routed, silently for now. A hint to the users is a
    later step of its own, since it needs an event the web UI understands.
  - **`max_route_decisions`** stays as a safety net against faulty routing.
  - **Waking:** a sleeping agent ignores a `ResponseRequested` (`AgentActor::handle_waiting`), so
    it has to be woken first. The rule is: a sleeping agent that is not explicitly addressed keeps
    sleeping; one that is, by a user or by an agent, is woken. `ChannelLoop::wake_mentioned` goes
    away; instead `route_event_impl` wakes every sleeping agent the moderator's decision sends a
    `ResponseRequested` to - after `moderator.route`, before the first envelope of the decision is
    delivered, and not when the channel is quiet and the request is not delivered. Since
    `actor_candidates` leaves sleeping agents out, only mentioned ones can be woken this way; a
    comment at both places says so. The rule of whose mentions count then lives in the moderator
    alone.
  - **`ExternalAgentActor::speak`** starts a fresh `EventContext`, which would start a chain at 0
    hops after a long task. The task keeps the context of the request that started it, and `speak`
    carries its hop count on.

  Order: first a restructuring without behaviour change (the field, waking from the decision),
  then the limit, routing agents' mentions, `speak`, and the tests (a chain that stops at the
  limit, a user message that starts a new one, a sleeping agent mentioned by an agent). That
  `speak` wakes every agent is issue [199](199-external-agent-speak-wakes-all.md).
- **2026-10-03** — The restructuring, without behaviour change for mentions: `EventContext` has
  `agent_hops` (kept by `next`, `#[serde(default)]` for stored turns), nothing counts it yet.
  `ChannelLoop::wake_mentioned` is gone; `route_event_impl` wakes everyone the moderator's decision
  asks to respond before it delivers anything of it. One difference: a mention in a quiet channel no
  longer wakes the mentioned agent, since nobody is asked there. CHANNELS.md (*Waking*) says so.
- **2026-10-03** — Done. `PolicyModerator` counts a hop for every `ResponseRequested` it sends for
  an agent's message, mentioned or chosen by the policy, and sets `agent_hops` on it; beyond
  `max_agent_hops` (parameter of the moderator component, default 3) it asks nobody and does not
  consult its policy, silently. `mentioned_responders` routes mentions by agents too; the senders
  stay left out. `OpenTask::requested` keeps the context of the answer to the request that began or
  took up a task, and `ExternalAgentActor::speak` continues it - in the request's span and chain.
  Agents learn the handles and the notation from `channel_members_prompt_provider` (176), where it
  is configured. Tests: the moderator asks agents an agent mentions, under every policy, one hop
  further; a chain ends at the limit while the message is still delivered, and a user's message
  starts a new one; the limit is a parameter; what an open task says unasked stays in its request's
  span. CHANNELS.md (*Chains between agents*), COMPONENTS.md, WEB-UI.md and the web UI's policy
  descriptions are updated. Open: a hint to the users when a chain is cut off.
- **2026-10-03** — The hint to the users when a chain is cut off is issue
  [200](200-chain-limit-hint.md).
