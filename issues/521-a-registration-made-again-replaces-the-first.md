# 521 — A component kind or a single registered again replaces the first without a word

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** Code structure
**Summary:** A component kind registered under a name that is taken, and a single declared again, replace the registration before without a word, so that a library registered after a plugin can change what every agent of the program is built of - its model among it.

## The problem

The `ComponentRegistry` keeps one registration per name of a kind and one declaration per single,
and the last one made wins:

- **A kind registered again** replaces the one of its name: `ComponentRegistry::add`
  (`caelumora-kernel/src/component/registry.rs:231`, `self.kinds.insert`) and the libraries'
  `ForLibrary::register` (`:731`) alike.
- **A single declared again** replaces the declaration before, its default kind with it
  (`single_of`, `:247`) - by design, as its doc comment says.

Plugins register before the libraries. Since issue
[520](520-the-kernel-sees-no-model-call.md) the model of an agent is the kind `model` of the plugin
`caelumora-models`, the default of both the model's facets, and no longer reserved for it: a
library may bring a model of its own, for the agents that configure it - that was decided. But a
library that registers a kind named `model`, or declares `ModelClient` a single with a default of
its own, changes the model of *every* agent of the program that names none - silently, and only
because it was registered later. The same holds for every kind and every single, not only the
model's: a misspelt or colliding name in a plugin goes unnoticed as well.

Found in the reviews of steps 3 and 4 of issue 520.

## Why it matters

What a program is built of should not depend on the order its plugins and libraries are listed in,
and a library, trusted for nothing (issue
[516](516-the-runtime-trusts-the-agents-linked-into-it.md)), should not decide the defaults of the
whole process. The sanctioned way to replace a default for every agent is the configuration
(`component_defaults`), which the operator writes.

## Considerations

- **Refuse a second registration.** A kind whose name is taken, and a single declared a second
  time, keep the kernel from being built - as `entity_store` refuses a kind of entity whose name is
  taken and `provide_of_library` a service offered twice. Simple, and it finds collisions of any
  origin.
- **Where a second declaration is meant.** A `TestKernel` declares a model that is not to be asked
  the default of the model's facets (`caelumora_kernel::testing::with_unasked_model`), which
  `ModelsPlugin` declares again where a test brings it; and a test that builds components without a
  kernel declares singles of its own. These would have to change: the test's stand-in declared only
  where no plugin declares the facet - registered after the plugins, with a call that declares
  where nothing is declared yet -, or given as the test's `component_defaults`, the way a program's
  configuration replaces a default.
- **Only libraries.** Refusing a second registration by a library alone keeps the plugins' freedom
  and the tests as they are, but leaves collisions between plugins unnoticed and makes the rule
  depend on who registers.

Recommended: refuse every second registration, and give the tests a way to declare only where
nothing is declared.

## Done when

- The kernel is not built where a kind is registered under a name that is taken or a single is
  declared twice, naming both and who made them.
- The tests' stand-in model and the tests without a kernel do what they did, without declaring a
  single a second time.

## History

- **2026-11-02** — Opened at the maintainer's request, from the reviews of steps 3 and 4 of issue
  [520](520-the-kernel-sees-no-model-call.md).
- **2026-11-03** — Done as recommended. Since issue 520 the names above are `language_model`,
  `caelumora-model`, `LanguageModelClient` and `with_unasked_language_model`. The registry keeps
  who registered each kind and declared each single (`Origin`: the kernel, a plugin, a library, a
  test), which replaces the mark of a library's (`of_library`, `singles_of_libraries`); the
  `KernelBuilder` names the plugin or library as it registers it. A kind registered under a name
  that is taken, or a single declared again, keeps the first and keeps the kernel from being built
  (`ComponentRegistry::registered_once`), naming who made both. A test's kernel registers its model
  that is not to be asked after its plugins and libraries, where none of them declares one; the
  tests without a kernel declared nothing twice and stay as they were.
