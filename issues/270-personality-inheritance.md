# 270 — Personalities repeat their components

**Status:** open
**Area:** Code structure

## The problem

A personality is a flat list of components (`Personality::components`, a `ComponentConfig` of
`ComponentDescriptor`s), and every personality writes the whole list out. What nearly every agent
needs - the roster (`channel_members_prompt_provider`), the language and the briefing of its
channel, the `stop_tool`, notes, the tools of the workspace - is repeated in each, and so is every
later addition: a command every agent should understand (`/compact`, issue
[269](269-agent-commands.md)) is added to each personality by hand, and a personality that was not
touched does not have it.

The one thing that is shared is `AppConfig::component_defaults`, which replaces the *default* of a
facet of which an agent has exactly one (`LLMClient`, `ContextProvider`, `ToolExecutor`). It says
nothing about the facets an agent has many of, and nothing a personality could build on.

## Why it matters

- Every cross-cutting addition - a prompt part everyone should have, a command, a tool - is a
  change to every personality, which nobody makes completely.
- Personalities that are meant to be alike drift apart, and which differences are intended cannot
  be told from the lists.
- The alternative taken case by case - a default set per kind of component in the configuration,
  as 269 considered for commands - is a mechanism per facet, none of which a personality can opt
  out of.

## Considerations

- **Inheritance between personalities.** A personality names one it extends (`extends: <scoped
  name>` or id); its components are the parent's followed by its own, resolved when the agent is
  built. Open with it: whether a child may remove or replace one of the parent's components (by
  id, say), how deep it goes, what a cycle or a missing parent does to the agents, and what the
  parent's `roles`, `reasoning_visibility` and `kind` mean for the child - a child of an external
  personality is external. The parent must be readable by whoever reads the child, or the child
  cannot be built: `GetPersonality` on both, in the agent's name.
- **A base configured once.** `AppConfig::personality_base`, a `ComponentConfig` every personality
  is built on top of - the `component_defaults` idea extended to every facet. Simpler, no lookup,
  no cycle; but one base for all, nothing to opt out of, and the base is configuration, not an
  entity anyone but the administrator can write.
- **Component bundles.** A named list of components, stored as an entity, that a personality
  includes by name among its components - several, in order. Finer than inheritance, and a bundle
  is a thing to show and reuse; but a new entity with ownership, scope and lookup of its own
  (issues 177, 178, 250), for what inheritance gives with one field.
- **Resolution is recorded, not recomputed.** What an agent was built from should be visible as
  the resolved list - the views, the web UI -, and a change of the parent reaches a running agent
  as a change of its own personality does today: not at all until it is built again.
- **Id collisions.** Two descriptors of one id in parent and child: a configuration error, as two
  tools of one name are (issue 37), or the child's replaces the parent's - which is the opt-out
  mechanism, if there is one. Decide once.
- The mechanisms are not exclusive: a base for the deployment and inheritance for the rest.

## Done when

- A way for a personality to share components with others exists, decided among the options
  above, and the built-in personalities of the seed use it.
- What an agent is built from is visible resolved, and an error in the sharing - a missing parent,
  a cycle - keeps the agent from starting with a message that names it.
- [COMPONENTS.md](../docs/COMPONENTS.md) describes it.

## History

- **2026-10-09** — Issue created, out of [269](269-agent-commands.md): the commands an agent
  carries out are configured per personality like its tools, and the chore of listing `/compact`
  in every personality is this issue's, not a default set for commands.
- **2026-10-09** — With [280](280-tool-executor-trait.md) the `ToolExecutor` is no facet any
  more; of the facets of which an agent has exactly one, `LLMClient` and `ContextProvider` remain.
