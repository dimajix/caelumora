# 337 — A container builds every component it is configured with, also those nobody asks for

**Status:** open
**Area:** Code structure

## The problem

`ComponentContainer::from_config` builds every entry of a configuration, whatever kind it is of
(`component.rs`, `with_defaults`). Its owner then asks for the facets it uses: a channel for its
`Moderator`, an agent for its `ContextProvider`, `LLMClient`, `ToolProvider`s and the rest. What
no one asks for was built all the same, and is never called.

Building is not free of consequences. A component is built in its owner's name (issue
[321](321-channel-components-in-the-owners-name.md)): it resolves a `ref` with `GetComponents`,
and a kind that uses the `SecretManager` resolves its secrets as it is built - `mcp_tool` connects
to its server with them, `a2a_agent` to its remote agent. An `mcp_tool` in a channel's components
thus reads its owner's secret and opens a connection, for a channel that never asks for a
`ToolProvider`. Nothing refuses the configuration; it does not even fail.

The same holds for every owner of a container: an external agent asks for no `ToolProvider`, a
session's command container only for `ChannelCommandProvider`s.

Found in the discussion of issue 321, where it is what is left of "what a component may read or
use" once the name it is built in is decided.

## Why it matters

What a configuration can set off should follow from what its owner does with it. A component
built for nothing uses its owner's rights for nothing, and a configuration that cannot work - a
tool in a channel - is accepted silently instead of being refused where it is written.

## Considerations

A list of the facets an owner asks for is not enough: components ask for others while they are
built. The `policy_moderator` asks for `one::<dyn ResponderPolicy>()`, the
`turn_context_provider` and the `external_context_provider` for `all::<dyn PromptProvider>()`.
The channel asks for no `ResponderPolicy`, and still needs one. What a container accepts is a
*closure*: the facets its owner asks for, and whatever the components built for them ask for in
turn.

### a) Kinds declare the facets they ask for (chosen)

`ComponentKind` gets `needs::<T>()` beside `provides` and `uses`.

- **The owner declares its facets.** `from_config` is given the facets its owner asks for, as a
  `&[TypeKey]` the owner keeps as a constant (a channel's `{ Moderator }`). The container refuses
  `one` and `get_all` of a facet not among them, so the declaration stays true.
- **The closure is the configuration's.** It starts at the owner's facets, takes the configured
  kinds that provide one of them - and the default kind of a single the configuration does not
  provide -, adds their `needs`, and repeats until nothing is added. Taken over the whole
  registry instead, every channel would accept what any registered moderator kind needs.
- **Checked before anything is built.** A configured component, a `ref` resolved, that provides
  no facet of the closure makes the configuration invalid, naming the component and its kind.
  Nothing is built then, and no secret read.
- **Enforced while building.** `ComponentContext::one` and `all` refuse a facet the kind did not
  declare, as `service` refuses a service it did not declare (`for_kind`).
- **Any number of a facet.** The rule asks which facet types are accepted, not how many components
  provide them: a `ToolProvider` in an agent's closure admits any number of tools; the check that
  a single has at most one stays as it is.
- **Several facets of one kind.** A kind is accepted if *one* of its facets is in the closure.
  `notes` provides a `ToolProvider` and a `PromptProvider`; in an external agent only the second is
  asked for, and the first, never called, does no harm. Requiring all of them would refuse it.

Example, a channel:

```
owner:     { Moderator }
  policy_moderator     provides Moderator, needs ResponderPolicy
closure:   { Moderator, ResponderPolicy }
  round_robin_policy   accepted
  mcp_tool             refused (ToolProvider), before a secret is read
```

**`uses` and `needs` stay two declarations.** Both say what a factory may ask its context for, by
type; but a service comes from the runtime, is the same for every graph and is always there, while
a facet comes from the container's own graph, is built on demand, and may be missing (`all` is
empty, `one` builds the default). Only `needs` takes part in the closure. One `uses` for both would
decide by `T` which of the two it means, which is written nowhere. They are built alike instead: a
builder of `ComponentKind` each, enforced in `for_kind`, refused with an error naming the kind and
`T`.

**"No `deps`" ([COMPONENTS.md](../docs/COMPONENTS.md#wiring)).** `needs` names facet types, as
`provides` does, never components: nothing is wired by hand, the container still finds who
provides a facet. What changes is that a `needs` can be forgotten - and a forgotten one is refused
loudly the first time the factory asks, naming the kind and the facet, as a forgotten `uses` is.

**Cost:** `needs` on the kinds that ask for others (`policy_moderator`, `turn_context_provider`,
`external_context_provider`, the test kinds of `component.rs`), and on those of plugins that do;
the facets of each owner of a container (channel, agent, external agent, session commands, the
containers the app and plugins build) given to `from_config`; the closure and the check in
`with_defaults`; COMPONENTS.md and PLUGINS.md.

### b) Build only what is asked for

`from_config` is given the owner's facets as roots and builds only them, and what their factories
ask for; whatever is left unbuilt is not built, and refused. It needs no declaration, and is exact
where a component asks for something only with some of its parameters.

Not chosen: whether a configuration fits is known only after building it. Neither a check before
the verb - creating a channel, storing a template - nor a web UI offering only the kinds that fit
can ask it, and it is a second pattern beside `uses`.

## Done when

- A container is given the facets its owner asks for, and refuses to be asked for others.
- A kind declares the facets it asks for with `needs`, and is refused one it did not declare.
- A configuration with a component that provides no facet of its closure is refused before
  anything is built; tests show it for a channel with a tool, and an agent with a moderator.
- COMPONENTS.md and PLUGINS.md describe `needs` and the closure.

## History

- **2026-10-13** — Opened from the discussion of issue 321; variant a) chosen.
