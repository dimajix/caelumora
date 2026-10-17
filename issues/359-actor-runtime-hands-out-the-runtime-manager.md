# 359 — `ActorRuntime` hands out the `RuntimeManager` to two actors that want the controllers

**Status:** done
**Area:** Code structure
**Epic:** [349](349-core-review-epic.md)
**Summary:** `ActorRuntime` hands out the `RuntimeManager`, which two actors use to reach past the facade for the `ControllerManager`.

## The problem

`ActorRuntime` is the narrow set of what an actor may reach ([69](69-scoped-actor-runtime.md)).
[191](191-channel-runtime-hands-out-runtime-manager.md) took `runtime_manager()` off the
`ChannelRuntime` and left it on `ActorRuntime` only for the views, which read through the manager
on purpose. Two actors reach past the facade with it:
`actor_runtime.runtime_manager().controller_manager()` in `UserActor::new` (`actor/user.rs:88`) and
`ApproverActor::new` (`actor/approval.rs:58`), to hold the `ControllerManager` as a field. Both
also hold the `ChannelController` and the `SecurityManager` beside the runtime that offers the
latter.

[229](229-services-fetched-through-the-runtime.md) is done with "no code outside the runtime module
reaches through `runtime_manager()` for a service" among its criteria; these two do.

## Why it matters

A facade that one caller may bypass limits nothing, and the next actor copies the line. What an
actor is allowed to reach should be readable from `ActorRuntime` alone.

## Considerations

- `ActorRuntime::controller_manager(&self) -> &Arc<ControllerManager>` among its accessors, as
  `security_manager` is; the two actors use it. If, after [350](350-lookups-written-out-again.md),
  they need only `agent_attributes`, that method may be the accessor instead.
- `ActorRuntime::runtime_manager()` becomes reachable for the views alone - `pub(in crate::view)`
  is not possible across modules, so either `ViewsFactory::of_actor` is the one caller and the
  method is documented as such, or the views take what they need in `of_actor`.
- Not in scope: the forwarders of `ChannelRuntime` ([89](89-channel-runtime-delegators.md),
  rejected).

## Done when

- No actor calls `runtime_manager()`; `ActorRuntime` offers what the actors need by name.
- No behaviour changes.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
- **2026-10-14** — Done. Both actors asked the `ControllerManager` one question,
  `agent_attributes`, so `ActorRuntime` offers that by name instead of the manager; the actors no
  longer hold a `controllers` field. `ActorRuntime::runtime_manager()` is gone altogether: the views
  stopped reading through it with [229](229-services-fetched-through-the-runtime.md), so its last
  user was a test, which restarts a channel with `ChannelRuntime::restarted()` (test-only) instead.
  The `SecurityManager` the actors hold beside their runtime stays, as constructor injection
  ([CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md)) asks.
