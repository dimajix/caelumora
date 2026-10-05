# 229 — Central services are fetched through the runtime again and again

**Status:** done
**Area:** Code structure

## The problem

A few services are needed by many types: the `SecurityManager` above all, the `EntityStore`, the
`StateStore`, the `EventBus`, the `ControllerManager`. Most types do not hold the ones they need but
the runtime (`RuntimeManager`, `ChannelRuntime`, `ActorRuntime`) or something leading to it, and
fetch the service at every use, often through a chain:

- `MemberView` asks five times
  `self.channel.views().runtime().security_manager().may_…(self.channel.views().viewer().context(), …)`;
  `.views().viewer()` appears ten times in the views.
- `SessionManager` and `SessionLoop` fetch `self.runtime_manager.entity_store()` 34 times and
  `event_bus()` 25 times in `session.rs`; `ChannelLoop` the `SecurityManager` 14 times.
- `ApproverActor` reaches `self.actor_runtime.runtime_manager().security_manager()` and
  `….controller_manager()` - through the crate-internal `runtime_manager()`, past the scoped
  `ActorRuntime` of [69](69-scoped-actor-runtime.md).
- Services are threaded as parameters into functions next to the object that holds them:
  `Member::of(…, &runtime.controller_manager(), runtime.security_manager().as_ref())`,
  `member.require_cleared(runtime.entity_store().as_ref(), runtime.security_manager().as_ref(), …)`
  (`channel.rs`, three times), `may_answer_in(actor_runtime.security_manager().as_ref(), …)`.
- Every accessor returns a new `Arc` (`RuntimeManager`, `ChannelRuntime`, `ActorRuntime`:
  `security_manager()`, `entity_store()`, `state_store()`, `event_bus()`, `secret_manager()`,
  `component_registry()`, `controller_manager()`), so each lookup also clones - an atomic
  increment, and an `.as_ref()` at every call that wants a `&dyn`.

How often each is fetched (`caelumora-runtime` and `caelumora-rest-server`, on 2026-10-05):

| Service | Lookups | Mostly in |
|---|---|---|
| `EntityStore` | about 65 | `session.rs` (34), the views (12 and more), `channel.rs` |
| `SecurityManager` | about 60, in eleven files | `channel.rs` (14), `session.rs` (10), the views (18) |
| `EventBus` | about 30 | `session.rs` (25) |
| `StateStore`, `ControllerManager`, `ComponentRegistry`, `SecretManager` | fewer than 15 each | spread out |

## Why it matters

- **Hidden dependencies.** A type holding the runtime can reach everything; its definition does
  not say what it needs or may do. That is a service locator, and it is the opposite of what
  [69](69-scoped-actor-runtime.md) and [128](128-actor-runtime-in-components.md) established for
  components: give each what it needs.
- **Heavy tests.** To test `ApproverActor` (issue 14), a whole `RuntimeManager` - stores, registry,
  secrets, commands - and a `ChannelLoop` had to be built, though it needs a policy, the agents'
  attributes and the users' contexts.
- **Noise at every check.** The chains make a permission check, the most important line of a
  method, the hardest to read.

None of these services changes while the process runs, so holding one cannot go stale - the one
reason to look a service up each time does not apply.

## Considerations

- **Constructor injection** is the usual practice in Rust: a type takes the services it needs in
  `new` and keeps them as fields, as `Arc<dyn Trait>` - the policy is chosen at run time
  (ownership, OPA in [24](24-opa-security-manager.md)), so `dyn` rather than a type parameter, and
  `Arc` rather than references in types that live in tasks. No global (`static`, `OnceLock`): it
  hides the dependency and binds every test to one instance.
- **Where it pays off first:**
  - the views: `Views` holds the `SecurityManager` and the viewer, and offers the questions in the
    viewer's name, so that `MemberView` asks `self.views().may_approve(…)` or similar;
  - `ApproverActor`: the policy and the controllers as fields, not through `runtime_manager()`;
  - `ChannelLoop`, `SessionLoop`, `SessionManager`: the services they use most as fields;
  - `Member::of` and `require_cleared` take what they need from one argument, not three threaded
    in next to it - or become methods of the object that holds them.
- **Bundles, not long parameter lists.** A type that needs four services takes a small context
  holding exactly those, as `ComponentContext` does; it must not become "everything" again.
- **Scoped runtimes stay.** `ActorRuntime` is an actor's scoped set of capabilities (issue 69) and
  keeps that role; what changes is that its users do not reach past it.
- **Accessors by reference.** `fn security_manager(&self) -> &Arc<dyn SecurityManager>` (or
  `&dyn SecurityManager`); whoever keeps one clones it.
- Not every lookup has to go: a type that needs a service once, rarely, and holds the runtime anyway
  gains nothing from another field.

## Done when

- The types that check permissions or use a store as their main work hold the services they need
  and no longer fetch them through the runtime at each use.
- No code outside the runtime module reaches through `runtime_manager()` for a service.
- The runtime's accessors return references.
- Done as refactorings without behaviour changes, in steps that can be reviewed one by one.

## History

- **2026-10-05** — Opened after [14](14-approval-privilege.md), whose approver reached the policy
  through `runtime_manager()` and whose tests needed the whole runtime. The maintainer proposed
  taking such services once, in the constructor; inventory as above.
- **2026-10-05** — The rules are in [CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md) ("Services
  are held, not fetched") and, in short, in `AGENTS.md`. Step 1, the views: `Views` takes the
  `SecurityManager`, both stores and the `ComponentRegistry` from the runtime once, in `new`, and no
  longer holds the runtime; it lends them (`security()`, `entities()`, `states()`, `components()`)
  together with the viewer's `context()`. `MemberView` reaches them through `views()`, so a check is
  `self.views().security().may_…(self.views().context(), …)`. No behaviour changed.
- **2026-10-05** — Step 2, the approver: `ApproverActor` takes the `SecurityManager` and the
  `ControllerManager` once, in `new`, and holds them, instead of reaching through
  `runtime_manager()` at every use. The `ControllerManager` still comes through `runtime_manager()`,
  which the `ActorRuntime` does not hand out on purpose ([69](69-scoped-actor-runtime.md)) - decided
  with the maintainer as better than before, not as the end: reading the attributes of an agent that
  asks from the running channel, which holds the controller its actor was built with, would need no
  `ControllerManager` at all. No behaviour changed. `runtime_manager()` is also still reached from
  outside the runtime module by `ViewsFactory::session`, which builds an agent's views, and by a
  test in `channel.rs`.
- **2026-10-05** — Step 3, the channel: `ChannelLoop` takes the `SecurityManager`, the
  `EntityStore`, the `ControllerManager` and the `EventBus` from its `ChannelRuntime` once, in
  `new`, and holds them; it keeps the `ChannelRuntime` only to build its actors' runtimes.
  `Member::of`, threaded three times with the controllers and the policy, is called through
  `ChannelLoop::member_of`. No behaviour changed. The free `channel::require_cleared`, which
  `SessionLoop` and `SessionManager` call for a new channel, still threads the runtime's services
  into `Member::of`; it goes with the step for the session.
- **2026-10-05** — Step 4, the session: `SessionLoop` and `SessionManager` take the
  `SecurityManager`, the `EntityStore` and the `EventBus` - about 70 of their lookups - from the
  `RuntimeManager` once, in `new`. They keep the `RuntimeManager`, which builds what they make:
  session loops, channel runtimes, views, the command executor. The few lookups of the `StateStore`
  and the `ControllerManager` stay, as do the module's helpers `require_classification`,
  `require_seen` and `channel::require_cleared`, which take the `RuntimeManager` whole rather than
  its parts. No behaviour changed.
- **2026-10-05** — Step 5, the views of an agent: `SessionLoop::views(viewer)` gives the views of an
  agent in its session, as `SessionManager::views` gives a user's, from the `RuntimeManager` the
  loop holds anyway (`Views::of_own_session`). `ViewsFactory` holds only the actor's runtime - or,
  in tests, the session loop - and no longer reaches through `runtime_manager()`. No behaviour
  changed. Left reaching through it: the approver for the `ControllerManager` (step 2, as decided)
  and a test in `channel.rs` that builds a channel anew after a restart and needs a fresh
  `ChannelRuntime`.
- **2026-10-05** — Step 6, the accessors: the accessors of `RuntimeManager`, `ChannelRuntime` and
  `ActorRuntime` lend `&Arc<…>` instead of a new `Arc` at every call; who keeps a service clones it
  with `Arc::clone`, which [CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md) now asks for
  ("Cloning shared pointers"). No behaviour changed. Not yet done: the code clones its `Arc`s with
  `.clone()` elsewhere - clippy's `clone_on_ref_ptr` finds about 350 places; enabling the lint and
  converting them is a step of its own. With this step the issue is done as far as it set out: the
  types named above hold their services, and the accessors lend.
- **2026-10-05** — Asked for by the maintainer, as part of this issue: the workspace turns on
  clippy's `clone_on_ref_ptr` (`[workspace.lints.clippy]`, every crate with `[lints] workspace =
  true`), and the code clones its `Arc`s with `Arc::clone` - about 350 places in 35 files, tests
  included. Where a concrete `Arc` is passed as an `Arc<dyn …>`, the type is named
  (`Arc::<MemoryStore>::clone(&store)`). No behaviour changed.
