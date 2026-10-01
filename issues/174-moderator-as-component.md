# 174 — The moderator is a name the runtime knows, not a component of the channel

**Status:** open
**Area:** Sessions and channels

## The problem

A channel's moderator is a string (`Channel::moderator`,
`ChannelTemplate.moderator`, default `round_robin`) that
`RuntimeManager::new_moderator` maps to one of four built-in
`ResponderPolicy`s in a `match`. Everything else the runtime builds per
participant - an agent's model, tools, memory, prompt parts - is a component,
configured and built through the `ComponentRegistry`. The moderator is the
exception:

- A new moderator means a change of the runtime; it cannot be configured,
  registered or given parameters (how many agents `random` picks, the order
  of `round_robin`, a moderator that asks a model whom to ask).
- Channels own no components at all, although
  [173](173-message-inspection.md) needs exactly that for its inspectors.

## Why it matters

- A moderator that decides by content - which agent fits the question
  best - needs a model, and models are components (since
  [130](130-llm-configuration.md)). As a name in a `match` it cannot have
  one.
- Two mechanisms for "something the channel is configured with" would
  diverge in naming, construction and error handling. Building the moderator
  the way inspectors will be built gives channels one way to be configured.

## Considerations

### What becomes the component

All moderators route the same way (every message to every other
participant, approvals and failures to whom they concern, mentioned agents
asked); they differ only in whom they ask when a message mentions no agent
(see [CHANNELS.md](../docs/CHANNELS.md#moderators),
[44](44-mentions-only-moderator.md), [114](114-moderator-details.md)).

- **(a) The `ResponderPolicy` is the facet.** `PolicyModerator` stays the
  runtime's, and with it the routing rules, mentions and the approver's
  share. A component decides only whom to ask. Nobody can configure a
  moderator that breaks routing or ignores mentions.
- **(b) The whole `Moderator` is the facet.** More freedom, but every
  configured moderator would have to route correctly itself - the rules
  that [29](29-mentions.md) and [162](162-ui-keeps-a-removed-agents-turn.md) rely on
  would no longer be the runtime's.

Recommended: (a). `ResponderPolicy::choose` is synchronous today; a policy
that asks a model needs it `async`, which the channel loop must then not
wait for inline - the same question as in [173](173-message-inspection.md).

### Components on the channel

Introduced here or by [173](173-message-inspection.md), whichever comes
first:

- the `ChannelTemplate` and the stored `Channel` name their components, and
  a channel created on request is given them;
- `Channel::build` / the `ChannelLoop` builds them through the
  `ComponentRegistry`, with a `ComponentContext` for a channel - there is no
  actor whose context it could be, so in whose name the components act has
  to be decided (`System`, or the channel's owner);
- an unknown or failing component is refused before anything is stored, as
  an unknown moderator is now (`SessionError::UnknownModerator`).

### Compatibility

- The four moderators remain, as preconfigured components under their
  current names, so that existing templates and stored channels keep
  working.
- The REST API returns `moderator` for every channel and accepts it when a
  channel is created; the web UI explains the four known kinds
  (`caelumora-web-ui/src/moderator.ts`) and shows any other by its name. That
  must stay so; a configured moderator could carry its own label and
  description, which the API would then return instead of the UI knowing
  them.

## Done when

- A channel's moderator is a component built through the `ComponentRegistry`,
  configured in the channel template or the request.
- `broadcast`, `round_robin`, `random` and `mentions_only` exist as such
  components; templates and stored channels that name them work unchanged.
- Routing and mentions stay the runtime's whatever moderator is configured.
- The web UI still shows and offers the moderator.
- [CHANNELS.md](../docs/CHANNELS.md) and [COMPONENTS.md](../docs/COMPONENTS.md)
  describe it.

## History

- **2026-10-01** — Opened together with [173](173-message-inspection.md),
  which needs components on the channel too.
- **2026-10-01** — [175](175-user-driven-moderator.md) shows that a
  `ResponderPolicy` has to know who wrote the message (`choose(sender,
  candidates)`); a policy as a component should get at least that.
