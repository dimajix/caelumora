# 201 — Components are written one level deeper than they need to be

**Status:** open
**Area:** Code structure

## The problem

Wherever components are written by hand - a personality, a session or channel template, a channel,
`commands` and `component_defaults` in the configuration - the list sits one level below the key
that names it:

```yaml
components:
  component_nodes:
  - id: responders
    kind: mentions_only_policy
```

The reason is only how serde maps a struct: `ComponentConfig` (`caelumora-model/src/component.rs`)
is a struct with the single field `component_nodes: Vec<ComponentDescriptor>`, and a struct is
written as a map. The level says nothing: there is nothing next to `component_nodes`, and nothing is
planned to be.

## Why it matters

- Every seed file, configuration example and document repeats the extra level, and anyone writing
  one by hand has to know it.
- In code, every access reads `….components.component_nodes`, which names the same thing twice.

## Considerations

- **Serialize `ComponentConfig` as its list.** `#[serde(transparent)]` (or `from`/`into` a
  `Vec<ComponentDescriptor>`) writes and reads `components:` directly as the list of descriptors.
  The defaults for fields left out by hand stay on `ComponentDescriptor`.
- **Stored data:** personalities, templates, channels and sessions in the `EntityStore` keep the old
  form. They may be deleted; no migration and no deserializer that accepts both forms (decided on
  2026-10-03).
- **The type itself:** once it is only a list, it is worth asking whether `ComponentConfig` still
  needs to be a type of its own (methods such as finding a node or the kind of a facet would argue
  for keeping it, as a newtype), and whether the field `component_nodes` is renamed or the newtype
  derefs to its list.
- **REST API and web UI:** wherever the API returns or takes components, they change form as well;
  the web UI has to be checked for readers of `component_nodes`.
- **What changes with it:** `examples/seed.yaml`, `examples/config.yaml`, `docs/CHANNELS.md`,
  `docs/CONFIGURATION.md` and the tests that write components by hand.

## Done when

- Components are written as a list directly under their key everywhere: configuration, seed files,
  stored entities and the REST API.
- Examples, documents and tests use the new form, and the web UI works with it.

## History

- **2026-10-03** — Issue created, after setting `max_agent_hops` in the seed's channels raised why
  `components` still holds a `component_nodes`. Decided: old stored data may be deleted, no
  migration and no transition deserializer.
