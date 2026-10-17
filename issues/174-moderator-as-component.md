# 174 — The moderator is a name the runtime knows, not a component of the channel

**Status:** done
**Area:** Sessions and channels
**Summary:** The moderator is a name mapped to four built-in policies, not a component of the channel, so it can be neither configured nor extended.

## The problem

A channel's moderator is a string (`Channel::moderator`, `ChannelTemplate.moderator`, default
`round_robin`) that `RuntimeManager::new_moderator` maps to one of four built-in `ResponderPolicy`s
in a `match`. Everything else the runtime builds per participant - an agent's model, tools, memory,
prompt parts - is a component, configured and built through the `ComponentRegistry`. The moderator
is the exception:

- A new moderator means a change of the runtime; it cannot be configured, registered or given
  parameters (how many agents `random` picks, the order of `round_robin`, a moderator that asks a
  model whom to ask).
- Channels own no components at all, although [173](173-message-inspection.md) needs exactly that
  for its inspectors.

## Why it matters

- A moderator that decides by content - which agent fits the question best - needs a model, and
  models are components (since [130](130-llm-configuration.md)). As a name in a `match` it cannot
  have one.
- Two mechanisms for "something the channel is configured with" would diverge in naming,
  construction and error handling. Building the moderator the way inspectors will be built gives
  channels one way to be configured.

## Considerations

### What becomes the component

All moderators route the same way (every message to every other participant, approvals and failures
to whom they concern, mentioned agents asked); they differ only in whom they ask when a message
mentions no agent (see [CHANNELS.md](../docs/CHANNELS.md#moderators),
[44](44-mentions-only-moderator.md), [114](114-moderator-details.md)).

- **(a) The `ResponderPolicy` is the facet.** `PolicyModerator` stays the runtime's, and with it the
  routing rules, mentions and the approver's share. A component decides only whom to ask. Nobody can
  configure a moderator that breaks routing or ignores mentions.
- **(b) The whole `Moderator` is the facet.** More freedom, but every configured moderator would
  have to route correctly itself - the rules that [29](29-mentions.md) and
  [162](162-ui-keeps-a-removed-agents-turn.md) rely on would no longer be the runtime's.

Recommended: (a). `ResponderPolicy::choose` is synchronous today; a policy that asks a model needs
it `async`, which the channel loop must then not wait for inline - the same question as in
[173](173-message-inspection.md).

### Components on the channel

Introduced here or by [173](173-message-inspection.md), whichever comes first:

- the `ChannelTemplate` and the stored `Channel` name their components, and a channel created on
  request is given them;
- `Channel::build` / the `ChannelLoop` builds them through the `ComponentRegistry`, with a
  `ComponentContext` for a channel - there is no actor whose context it could be, so in whose name
  the components act has to be decided (`System`, or the channel's owner);
- an unknown or failing component is refused before anything is stored, as an unknown moderator is
  now (`SessionError::UnknownModerator`).

### Compatibility

- The four moderators remain, as preconfigured components under their current names, so that
  existing templates and stored channels keep working.
- The REST API returns `moderator` for every channel and accepts it when a channel is created; the
  web UI explains the four known kinds (`caelumora-web-ui/src/moderator.ts`) and shows any other by
  its name. That must stay so; a configured moderator could carry its own label and description,
  which the API would then return instead of the UI knowing them.

## Done when

- A channel's moderator is a component built through the `ComponentRegistry`, configured in the
  channel template or the request.
- `broadcast`, `round_robin`, `random` and `mentions_only` exist as such components; templates and
  stored channels that name them work unchanged.
- Routing and mentions stay the runtime's whatever moderator is configured.
- The web UI still shows and offers the moderator.
- [CHANNELS.md](../docs/CHANNELS.md) and [COMPONENTS.md](../docs/COMPONENTS.md) describe it.

## History

- **2026-10-01** — Opened together with [173](173-message-inspection.md), which needs components on
  the channel too.
- **2026-10-01** — [175](175-user-driven-moderator.md) shows that a `ResponderPolicy` has to know
  who wrote the message (`choose(sender, candidates)`); a policy as a component should get at least
  that.
- **2026-10-01** — Done. Decided with the maintainer, differing from the recommendation above: the
  moderator itself is a component (facet `Moderator`, kind `policy_moderator`), and the
  `ResponderPolicy` is its slot, a facet of its own that the `policy_moderator` asks for
  (`context.one`). Both are facets of which a channel has exactly one, with the defaults
  `policy_moderator` and `round_robin_policy`. The policies are named `broadcast_policy`,
  `round_robin_policy`, `random_policy` and `mentions_only_policy` and stay in `moderator_impl/`.
  `Channel` and `ChannelTemplate` have `components` instead of `moderator`; the `ChannelLoop` builds
  them itself, like the `AgentActor`, in the name of the channel's owner -
  `RuntimeManager::new_moderator` is gone, and components that make no moderator are found when the
  channel is built (`SessionError::InvalidModerator`), after which it is taken back out.
  `ResponderPolicy::choose` is async, takes `&self` and the sender
  ([175](175-user-driven-moderator.md)). The REST API takes and returns `moderator` and
  `responder_policy` as kinds (`null` for a preconfigured component, `ComponentRegistry::kind_of`);
  the web UI explains the policy. Stored channels and templates naming a `moderator` fall back to
  the defaults; `examples/seed.yaml` is updated.
- **2026-10-01** — The channel keeps its moderator, and the `policy_moderator` its policy, as the
  `ComponentImpl` that `one` returned, like the agent keeps its components, instead of only its
  facet; [COMPONENTS.md](../docs/COMPONENTS.md#wiring) states this for every holder of a component.
  `ComponentImpl` dereferences to its facet, so that the facet's methods are called on it without
  `.facet`.
