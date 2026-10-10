# 288 — An external agent can be given a compaction it cannot have

**Status:** open
**Area:** Integrations

## The problem

The component `compaction` ([16](16-context-compression.md)) carries out `/compact` with the
agent's own model, which its factory takes from the container (`context.one::<dyn LLMClient>()`).
An external agent ([179](179-external-agents-via-a2a.md)) has no model of its own here - but the
`LLMClient` is a facet of which a container has exactly one, with `ollama_client` as the
registry's default, so a container that is asked for one and configures none builds the default.
A personality of kind `external` that lists `compaction` therefore builds without complaint, gets a
model nobody chose, and `/compact` then summarises what its transcript holds on the active path:
what the remote agent is to be given with its next call (issue 205), not what it keeps. The remote
agent's context is not shortened at all, and what it is given next is a summary written by another
model in place of what was said.

Nothing refuses it. The example seed is not affected - its external personality does not include
`agent-basics` -, but nothing keeps a personality from listing it.

## Considerations

- **No default model for an external agent.** The external agent's container already gets defaults
  of its own (`ComponentContainer::with_defaults`, issue 214: its `ContextProvider`). If it can
  say "no `LLMClient` unless configured", the `compaction` factory fails with the existing error
  of a single facet that has no default, and the agent does not start - the way a personality
  that lacks its `ExternalAgent` fails (issue 179). Cheapest, and it also stops any other component
  from quietly getting a model an external agent does not have. Whether `with_defaults` can
  express "none" is to check.
- **Components declare which kind of agent they serve.** `ComponentKind` could say that it is for
  agents run here, or for both, and building a personality's container refuses one that does not
  fit its kind. More general - `create_plan_tool` and the tool providers mean nothing to an external
  agent either, and are built for it anyway - but a new concept in the registry; weigh it against
  what it would refuse besides `compaction`.
- **A compaction the remote side understands.** Issue 269 left `/compact` of an external agent to
  its component: `a2a_agent` could provide the command itself if its protocol had a word for it.
  A2A has none today. That is a feature, not the fix for this issue.

## Done when

- A personality of kind `external` that lists `compaction` is refused when its agent is built, with
  an error that names why, instead of compacting what the remote agent is to be given next.

## History

- **2026-10-10** — Issue created from [16](16-context-compression.md), where it was noticed while
  building `/compact`.
- **2026-10-10** — How a context is compacted moved into the facet `CompactionStrategy` (see
  [284](284-automatic-context-compaction.md)): the model is now taken by its default,
  `summary_compaction`, and `compaction` takes the strategy. The problem stays; one more way to
  solve it is open: the external agent's container (`with_defaults`) gives no default
  `CompactionStrategy`, so `compaction` fails to build there, while the `LLMClient` stays as it is.
