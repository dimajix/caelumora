# 45 — Mentions written by agents are not routed

**Status:** open
**Area:** User interaction

## The problem

An `@`-mention written by a user makes the mentioned agents respond
([29](29-mentions.md)). A mention written by an agent does nothing: it is
resolved and highlighted, but `ModeratorActors::mentioned_responders` returns
`None` for any sender that is not a user. An agent cannot hand a question to
another agent (`@researcher, can you look this up?`). This holds for every
moderator, including `mentions_only` ([44](44-mentions-only-moderator.md)),
where agents therefore never answer each other at all.

Mentions by agents were left out on purpose. Two agents addressing each other
would talk indefinitely, and nothing bounds such a chain today in a way that
reflects what happened.

## Why it matters

- Delegation between agents is the reason to put several specialised agents
  into one channel. Without it the user has to relay every question by hand.
- `mentions_only` makes the gap obvious: it is the moderator where addressing
  is the only way to get an answer.

## Considerations

### What has to be tracked

To bound a chain, the channel must know, for an agent's message, how many
agent-to-agent hops lie between it and the last message a user wrote. A user
message starts a new chain; every agent answer that was triggered by an agent
mention extends it.

### What exists: `span_id` and `span_depth`

`EventContext` already carries `span_id` and `span_depth`
(`caelumora-model/src/actor.rs`). `EventContext::next` keeps the span and
increments the depth, and `ChannelLoop::route_event_impl` sets both on every
forwarded envelope and stops routing at `max_route_decisions` (a hard-coded
100), with only a `warn!`.

That is the right idea, but it measures something else:

- `span_depth` counts *routing steps*: forwards, `ResponseRequested`, the
  events of a turn. One user message followed by one answer already consumes
  several steps, so the depth says little about how many agents took part.
- The limit is a safety net against runaway routing, not a rule users can
  understand or configure, and hitting it is silent.

Options:

1. **Count agent hops in the span.** Add a field, e.g. `agent_hops`, to
   `EventContext`. It is reset when a user message starts a span and
   incremented when an agent's mention makes another agent respond. The
   moderator routes an agent mention only while it is below a limit. This
   reuses the span that already ties a chain together, and the count travels
   with the envelope.
2. **Derive it from `span_depth`.** No new field, but the limit would depend on
   how many events a turn produces, which changes whenever the agent loop
   changes.
3. **Track it in the moderator.** The moderator remembers per span how many
   agent hops it has routed. That needs no change to `EventContext`, but it is
   state that is lost on restart and has to be cleaned up.

Option 1 looks cleanest. It also needs the span to begin at the user's message
and to survive the agent's turn up to its `FullResponseGenerated`, which has to
be verified.

### The limit

- **Where it is configured:** per channel in the template, next to the
  moderator, with a default (e.g. 3), or globally.
- **When it is reached:** the mention is not routed. Whether users are told
  (a hint, like a notice) or it happens silently has to be decided; silently is
  what `mentions_only` does for a message without a mention.
- **What it replaces:** whether `max_route_decisions` stays as a separate
  safety net or is replaced.

### What agents need to know

Agents only use mentions if they know the notation and the channel's handles.
That fits a `SystemPromptProvider` contribution listing them. An agent also
needs to see in its context that a message was addressed to it, and by whom.

### Self-mentions and mentions of the sender

An agent mentioning itself, or the agent that addressed it, must not create a
loop that the limit alone stops. `mentioned_responders` already skips the
senders of the messages; whether that is enough across hops has to be checked.

## Done when

- A mention written by an agent makes the mentioned agents respond, under
  every moderator, up to a limit on consecutive agent-to-agent hops since the
  last user message.
- It is decided and documented how the hops are counted, where the limit is
  configured and what happens when it is reached.
- Agents learn the channel's handles through their system prompt.
- Tests cover a chain that stops at the limit and a user message that starts a
  new chain.

## History

- **2026-09-25** — Issue created, split off from
  [44](44-mentions-only-moderator.md). The idea to track the chain through the
  span (`span_id` plus a hop count) comes from the discussion there.
- **2026-10-01** — [175](175-user-driven-moderator.md) asks for a moderator
  under which agents' messages make nobody respond; its only exception, an
  agent's mention, is this issue.
