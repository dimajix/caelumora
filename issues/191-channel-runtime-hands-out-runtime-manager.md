# 191 — The channel runtime hands out the whole `RuntimeManager`

**Status:** open
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
