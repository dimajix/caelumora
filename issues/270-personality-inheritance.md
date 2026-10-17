# 270 — Personalities repeat their components

**Status:** done
**Area:** Code structure
**Summary:** Every personality writes out its whole list of components, so what every agent should have is repeated in each and a later addition reaches only those touched by hand.

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

## Decision

Neither inheritance nor a base in the configuration, and no bundle as an entity of its own: the
preconfigured components that `kind: ref` already includes become lists. A `StoredComponent` holds
a `ComponentConfig` instead of one descriptor - a single preconfigured component is a list of one -
and a `ref` stands, where it is written, for all of its descriptors, in their order.

1.  What the `ComponentStore` already gives is what sharing needs: an owner and contributors, its
    own actions (`GetComponent` and the others), a read afresh at every build in the name of the
    agent - so a change reaches an agent when it is built again, and a bundle the agent may not
    read keeps it from being built -, and no ref within a ref, so no cycle and no depth to bound.
2.  It serves every `ComponentConfig`, not only personalities: a channel's components and a
    session's commands can share the same way.
3.  The ids are decided once: a ref keeps an `id` of its own, as every descriptor does, unique
    among the entries of its list; the components a list resolves to have unique ids, too. A
    component of a bundle that the personality writes again is a configuration error that names
    the ref (as two tools of one name are, issue 37) - not an override. Opting out is not
    including a bundle; the bundles are cut finely enough for that.
4.  A personality's `kind`, `roles`, `reasoning_visibility` and `introduction` are not shared; a
    bundle is components only.
5.  The example seed gets a section `components` of its own; the built-in personalities include
    the bundles `channel-context` and `agent-basics` from it.

Considered and not taken: `extends` (a field, but rules for every other field of a personality,
cycles, a parent that is a selectable personality of its own, and only for personalities);
`AppConfig::personality_base` (one per `PersonalityKind` at least, since an external agent builds
every component it is given and needs `changes_only`; nothing to opt out of, the administrator's
alone, and a second mechanism beside `component_defaults`); a bundle entity of its own (a second
`ComponentStore`).

## Stored data

A `StoredComponent` is no longer written as a descriptor with its attributes beside its fields, but
as `{ id, components: [...], security_attributes }`. The stored preconfigured components (the
`components` directory beside the entity store) have to be deleted.

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
- **2026-10-09** — Decided (see *Decision*): the preconfigured components of `kind: ref` become
  lists, which personalities include as bundles; work started on the branch
  `issue-270-component-bundles`.
- **2026-10-09** — Implemented on the branch: `StoredComponent` holds a `ComponentConfig`;
  `ComponentRegistry::resolve` gives a `ref`'s whole list, `ComponentContainer::with_defaults`
  places it where the `ref` is written and refuses a component brought twice, naming the `ref`. The
  seed has a section `components` (`channel-context`, `agent-basics`), which `jonathan-assistant`
  and `sassy-assistant` include; `hermes-remote` keeps its context written out for `changes_only`.
  The security suite records that the included components are read in the agent's name (P7).
  Still open: what an agent is built from is not visible resolved; and a list its owner may no
  longer read - a share withdrawn, a list deleted - does not leave the agent out of its channel,
  as a withdrawn personality does (issue 191), but keeps the whole channel from opening, so that
  the list's owner can stop someone else's session.
- **2026-10-09** — Both open points resolved without `Member::of` resolving anything: a list that
  cannot be read fails as `ComponentError::Unavailable`, which the channel counts as an agent that
  cannot run, so it is left out and the channel starts (P7). What an agent was built from is
  recorded by its container (`ComponentContainer::components`), kept by its actor and read through
  `MemberView::components` and `GET …/actors/{aid}/components`, by its owner alone - the new
  action `ReadAgentComponents`, since what it includes may have been shared with the owner alone.
  The web UI has the flag `can_read_components`, but shows nothing of it yet.
- **2026-10-09** — Named in the plural, without a change in behaviour: `StoredComponent` became
  `StoredComponents`, the operations of the `ComponentStore` `create_components`, `get_components`,
  `update_components` and `delete_components`, the actions `GetComponents`, `CreateComponents`,
  `UpdateComponents` and `DeleteComponents`, and `EntityKind::Component` `Components`. Folding
  `StoredComponents` into `ComponentConfig` was weighed and not done: a `ComponentConfig` held by
  a personality, a channel or a template would carry an id and attributes that mean nothing there
  (issue 177), and could no longer be written as a bare list (issue 201).
- **2026-10-09** — Done. Showing what an agent was built from in the web UI went to issue
  [283](283-show-agent-components.md); the runtime and the REST server answer it.
