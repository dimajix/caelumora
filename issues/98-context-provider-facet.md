# 98 — `ContextProvider` is a facet with one implementation and undefined behaviour for two

**Status:** done
**Area:** Context & knowledge

## The problem

`AgentActor::next_turn` (`actor_impl/agent.rs:201-214`) iterates `Vec<ComponentImpl<dyn
ContextProvider>>`; each calls `Turn::set_context` and overwrites the previous one. The one
implementation, `StateContextProvider`, collects the `PromptProvider`s from its `deps` - the known
stumbling block (comment `tui/main.rs:40`, doc `create_plan_tool.rs:42-44`), and the very thing
issue [32](32-unavailable-tools-invisible-to-model.md) criticises: a forgotten `deps` entry loses a
prompt part silently. Issue [56](56-response-language.md) asks whether a prompt part is "built in or
a default component" - the same question.

## Why it matters

Two `ContextProvider`s are a configuration that builds and then silently shows the model half its
context. And the facet is a level of indirection between the actor and the prompt parts that exists
for one class.

## Considerations

- **(b) Remove the facet.** The actor collects `get_all::<dyn PromptProvider>` itself and builds the
  `ContextItem`s as `StateContextProvider::current_context` does today; order = component order in
  the configuration. `context.rs` loses the trait, `state_context_provider.rs` goes, the configs in
  `tui/main.rs` and `rest-server/data.rs` lose the node and its `deps`, ARCHITECTURE.md changes.
  Recommended, unless a second provider is foreseeable.
- **(a) Enforce cardinality.** `from_config` or the actor requires exactly one `ContextProvider`,
  else `InvalidConfig`. Smallest change, keeps the indirection.
- Issue [16](16-context-compression.md) is *not* an argument for (a): compression produces a new
  transcript with a provenance relation - an operation on the transcript, not a second supplier of
  context.

This is an architecture decision and is discussed before it is implemented. Followed by
[99](99-component-factory-and-tags.md).

## Done when

- A configuration cannot lose a prompt part silently.
- Either the facet is gone, or exactly one provider is enforced - and ARCHITECTURE.md says which and
  why.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 4.1.1).
- **2026-09-29** — Discussed with the maintainer, who wants the agent assembled from components like
  a Spring Boot application rather than fewer components: neither (a) nor (b). The `ContextProvider`
  stays a component, as a facet of which there is exactly one, with the default kind
  `state_context_provider`, which has every `PromptProvider` injected by type instead of through
  `deps` - the mechanism is issue [99](99-component-factory-and-tags.md). Each `PromptProvider`'s
  own `fail_policy` is to be honoured (today a failing part fails the turn, whatever its policy).
  The executor follows as issue [127](127-tool-executor-not-replaceable.md).
- **2026-09-29** — Done. `ContextProvider` is the first facet of which an agent has exactly one
  (`ComponentRegistry::single` in `component_impl::register_all`, default kind
  `state_context_provider`): the agent takes it with `components.one::<dyn ContextProvider>()`, a
  personality with two is refused before anything is built, and one with none gets the default - so
  the `context` nodes are gone from `seed.yaml`. `AppConfig::component_defaults` can replace the
  default for all personalities (a test checks the example of `examples/config.yaml` and that a
  component that provides no context is refused). `StateContextProvider` has every `PromptProvider`
  injected with its component's id and fail policy, and honours the policy: a part that fails open
  is left out of the turn with a warning, one that fails closed fails the turn, named. The
  `ContextProvider` itself fails the turn whatever its own policy - without a context there is
  nothing to give the model. ARCHITECTURE.md says so under the prompt parts.

