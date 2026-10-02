# 200 — Users are not told when a chain between agents is cut off

**Status:** open
**Area:** User interaction

## The problem

Since [45](45-agent-mentions.md), the `policy_moderator` asks nobody to respond once a chain
between agents would exceed `max_agent_hops`. It does so silently: the message is delivered, a
`debug!` line is logged, and nothing reaches the users. To them an agent that wrote
`@researcher, can you look this up?` at the end of a chain looks ignored, and a discussion between
agents simply stops without a reason.

Silence was chosen on purpose for the first step of 45, because a hint needs an event the web UI
understands.

## Why it matters

- A user cannot tell a cut-off chain from an agent that did not want to answer, or from a fault.
- Knowing it, the user can carry on the chain with a message of their own, which starts a new one.

## Considerations

- **Which event:** the users are already told of things that are not part of the history by
  `TurnFailed` and `CommandExecuted`, which `route_to_users` delivers and the web UI shows as a
  system notice that is gone on reload. A hint could be a new `ActorEvent` (e.g. a notice with a
  kind, so that it is not a third special case), or reuse one of these. `TurnFailed` would be
  wrong: no turn failed. A general notice event would also serve other hints of the channel.
- **Who sends it:** the decision is the moderator's, but a moderator only routes events, it does
  not send any of its own. Either `RouteDecision` gets a way to carry a notice that the channel
  sends in its own name (`ActorId::System`), or the channel recognises the cut-off itself. The
  first keeps the knowledge in the moderator.
- **Whom it reaches:** the users of the channel, not the agents - an agent told that it was not
  answered might try again.
- **What it says:** which agents would have been asked, and that a message of a user continues.
- **Compatibility:** a new event kind must be ignored by an older web UI, not break it; the REST
  stream passes envelopes through as they are.

## Done when

- When the moderator cuts a chain off, the channel's users see a notice that says so, and the
  agents do not.
- The web UI shows it like the other notices.
- A test covers the notice at the limit and its absence below it.

## History

- **2026-10-03** — Issue created, split off from [45](45-agent-mentions.md), which decided to cut
  chains off silently at first.
