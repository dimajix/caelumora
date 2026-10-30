# 449 — The runtime is given the policies twice, in its stores and beside them

**Status:** done
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** `RuntimeBuilder::new` and `RuntimeManager::new` take the `AccessPolicy` and the `ConfidentialityPolicy` beside a `Persistence` that was made with them, and nothing but a doc comment says that they have to be the same instances - tests already run with stores that permit everything under a runtime that checks.

## The problem

The stores check every access to an entity by the policies they were made with
(`Persistence::new(entities, states, access_policy, confidentiality)`, issues
[446](446-stores-check-access-each-and-are-enumerated.md) and
[448](448-persistence-inherits-the-state-stores.md)). The runtime is given the policies once more:
`RuntimeBuilder::new(persistence, secret_manager, access_policy, confidentiality, ...)`, and
`RuntimeManager::new` likewise. It hands them to the event bus, the managers, the controllers, the
participation, the command and tool executors, the views and, as services, to the backends and the
components of the plugins - some twenty users. Its doc comment says that `access_policy` "must be
the same instance the stores were built with"; nothing makes it so.

`RuntimeBuilder::from_config` keeps to it, the tests do not:

- `TestRuntime` has `store`, `access_policy` and `confidentiality` as fields of their own, each with
  a default. A test that names a policy and leaves the store at its default, or makes its store with
  `Persistence::in_memory()`, runs a runtime that checks under that policy over stores that permit
  everything - `caelumora-agents/src/components/agent_memory.rs` and `tool/mod.rs`, the helpers
  `agent_channel_hearing` (`caelumora-agents/src/channel_tests/mod.rs`) and `channel_with`
  (`caelumora-runtime/src/channel/tests/mod.rs`), `actor/approval.rs`, among others.
- The test of the program (`caelumora-app/src/main.rs`) makes the confidentiality policy twice
  from the configuration, one for its stores and one for the runtime.

What is tested there is a setup that does not exist in production.

## Considerations

- **A - the runtime takes the policies from the `Persistence`**, which would offer
  `access_policy()` and `confidentiality()`; the two parameters go. The difference cannot arise any
  more, but a service of the whole process would be taken, for its twenty users, from one of them:
  the stores would be the source of the policy they only check against.
- **B - the runtime makes the `Persistence`** - chosen on 2026-10-22, replaced by D on 2026-10-30:
  `RuntimeBuilder::new` takes the backend of the entities and the backend of the states,
  `Arc<dyn PersistenceBackend>` each, instead of a `Persistence`, and makes it of them and the
  policies it is given and hands out. Who hands the policy to everyone hands it to the stores, too,
  and there is one way it comes in.
- **C - the runtime checks** that the stores were made with the same instances
  (`Persistence::is_checked_by`, `ptr::addr_eq`) and refuses others. Tried on 2026-10-22: about 225
  tests failed - about 100 of them with alike but distinct instances (a store of
  `Persistence::in_memory()` beside the default policies of `TestRuntime`), about 127 with a real
  difference (the runtime under `OwnershipPolicy` or a policy of the test, the stores under
  `AllowAllPolicy`). It guards the difference instead of removing it, and keeps the parameters
  twice; taken back.
- **D - the stores know no policy; the runtime checks them by its own.** `Persistence` keeps the
  entities and the states, and also checks them by the policies it was made with - two concerns.
  It would keep only the first: `Persistence::new(entities, states)` and
  `Persistence::in_memory()` take no policies, `in_memory_with` goes. A store is checked by the
  policies given where it is opened, in one method: `Persistence::entity_store::<T>(&policies)`,
  through which `EntityStores::open(&persistence, &policies)` opens the stores of the core's kinds
  and `RuntimeBuilder::entity_store` those of a plugin's. `RuntimeBuilder::new` takes the
  `Persistence` and the policies, as now, and opens its stores by its own. `Policies`, the pair of
  the two, becomes a type of its own; `EntityStoreFactory`, whose one implementation is the
  `Persistence`, goes. The policies come in at one place, and the stores have none of their own to
  differ by. Unlike B, `from_config` keeps its order - policies, secrets, backends,
  stores -, the runtime is still given a `Persistence`, and `TestRuntime` keeps its fields, which
  cannot contradict one another any more. What B costs is mostly the same for D: the registry,
  the memberships and the tests that write before the runtime exists get the stores otherwise (see
  below). Its weak spot: whoever holds a `Persistence` can open stores checked by any policies - a
  test its fixtures under `AllowAllPolicy` -, but says so where it opens them, and the runtime never
  runs under other policies than its own.

What B costs: whatever needs the stores before the runtime is made has to get them otherwise.

- `RuntimeBuilder::from_config` binds the memberships to the store of the groups
  (`memberships.bind`) and makes the component registry of the component store
  (`ComponentRegistry::built_in(persistence.component_store())`), which `new` is given. The registry
  could be made by `new` from its own `Persistence`, with what a test adds to it given otherwise;
  the memberships bound after `new`, from the builder's stores.
- `TestRuntime` loses `store` for the backends - or keeps a backend of memory as its default - and
  tests that write into the stores before the runtime exists write into them through the runtime or
  the builder (`RuntimeBuilder::entity_store`, `RuntimeManager::entity_stores`).
- The tests that ran under two policies are given one; some may then fail at what they prepare,
  which the stores now check, too.

The `SecretManager`, too, is given the `AccessPolicy` beside the runtime; it is made before the
stores in `from_config`, since their backends may need a secret to connect (issue
[447](447-store-seam-not-fit-for-a-database.md)). It is not part of this issue, but the same
question.

## Decided

With the maintainer, on 2026-10-22: B. How the registry, the memberships and the tests get the
stores is to be laid out before it is built.

With the maintainer, on 2026-10-30: D instead of B, the policies given where a store is opened
(`Persistence::entity_store`, `EntityStores::open`), without a type of checked stores beside the
`Persistence`.

In two steps:

1. **The runtime makes its component registry** of its own stores, without a change of behaviour:
   `RuntimeBuilder::new` is given no registry, but makes the built-in one of the store of the
   bundles it opened; what a test adds to it, it adds through `RuntimeBuilder::components`, as a
   plugin does (`TestRuntime::components`). `RuntimeManager::new`, whose one caller is
   `TestRuntime`, goes; `TestRuntime` builds through the `RuntimeBuilder`.
2. **D.** `Policies` a type of its own, which `RuntimeBuilder::new` takes in place of the two
   policies; the memberships bound after `new`, to the builder's stores. A test that writes what it
   needs before the runtime exists opens the stores by policies it names - those that permit
   everything, `Policies::permitting_all()` of the model's `testing`, for most -, not through the
   runtime: the change stays mechanical, and the runtime checks by its own all the same. Writing
   them through the runtime would be one way into the stores, but means turning the helpers of
   some seventy files around, and the tests of the model and of the backends, which have no
   runtime, need the first way anyway.

## Done when

- A `Persistence` holds no policies; every store opened of it is checked by the policies given
  where it is opened, and the runtime opens its own by the policies it is given.
- No test runs a runtime over stores checked by other policies than the runtime's.
- `docs/STORAGE.md` and `docs/ARCHITECTURE.md` say so.

## History

- **2026-10-22** — Opened from the review of [448](448-persistence-inherits-the-state-stores.md).
  C was tried first and taken back, since about 225 tests failed by it; B is to be done instead.
- **2026-10-30** — D proposed: the stores keep no policies, and the runtime checks them by its
  own, so that only the checking moves into the runtime, not the making of the stores. Chosen
  instead of B the same day, with the policies given where a store is opened rather than bound in a
  type of checked stores.
- **2026-10-30** — Step 1 done: `RuntimeBuilder::new` makes the built-in registry of the store of
  the bundles it opened, a test adds its kinds through `TestRuntime::components`, and
  `RuntimeManager::new` is gone. The tests of an external agent and of the texts of a model's
  variant no longer register the agent's kinds themselves beside the plugin that registers them:
  they run `TestAgents`.
- **2026-10-30** — Step 2 done, and with it the issue: a `Persistence` is made of its backends
  alone, and a store is checked by the `Policies` it is opened by (`Persistence::entity_store`,
  `EntityStores::open`); `RuntimeBuilder::new` takes the `Policies` and opens its stores by them.
  `EntityStoreFactory` and `Persistence::in_memory_with` are gone. The memberships are bound to
  the runtime's store of the groups. What a test writes before its runtime exists, it writes
  through stores opened by `Policies::permitting_all()`, or by the policies it tests
  (`Policies::by_default_rules`); what it writes once the runtime exists, through the runtime's
  stores - the `World` and `owned_runtime`, which now gives the runtime beside its manager, as
  well. The `World` opens a plugin's kind by its policies (`World::entity_store`). A kind opened
  again is a store over the same storage, no longer the same instance.
