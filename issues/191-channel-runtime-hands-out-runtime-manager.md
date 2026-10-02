# 191 — The channel runtime hands out the whole `RuntimeManager`

**Status:** done
**Area:** Code structure

## The problem

`ChannelRuntime::runtime_manager()` became `pub(crate)` so that `Member::of` and
`Member::require_cleared` - called from `ChannelLoop` and from `session.rs` - can take a
`&RuntimeManager`. `ChannelLoop` now reaches through its runtime for it at several call sites. The
narrow accessors `ChannelRuntime` offered before existed to avoid exactly this
(issue [144](144-runtime-and-entity-accessors.md)).

## Why it matters

A facade that hands out what it wraps no longer limits what the channel can reach.

## Considerations

- `Member::of` and `require_cleared` take only what they use: the `EntityStore`, the
  `SecurityManager` and the `ControllerManager`.
- Or a small trait both `ChannelRuntime` and `RuntimeManager` implement.

## Done when

- `ChannelRuntime` does not hand out the `RuntimeManager`.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series.
- **2026-10-02** — Done, as decided with the maintainer: neither of the two considerations, but the
  first with the personality given an owner. `AgentController::personality` reads the agent's
  personality, so that `Member::of` takes the `ControllerManager` and the `SecurityManager`, and
  `require_cleared` the `EntityStore` and the `SecurityManager`. `ChannelRuntime::runtime_manager`
  is gone, and `ChannelRuntime::entity_store` joins its other narrow accessors. Like the agent's
  document, the controller reads the personality in the runtime's name; whether the agent may run
  with it is the channel's own check, `Enforcement::require` with `GetPersonality` in the agent's
  name, where it was hidden in reading the store before - a controller is the runtime's and grants
  nothing. A trait over both runtimes was rejected: it would only bundle accessors. `ActorRuntime`
  still hands the `RuntimeManager` to the views factory; that is deliberate - `Views` read through
  the runtime manager - and stays.
