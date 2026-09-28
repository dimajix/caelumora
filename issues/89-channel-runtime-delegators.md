# 89 — `ChannelRuntime` is a layer of delegators without a life of its own

**Status:** open
**Area:** Code structure

## The problem

`caelumora-runtime/src/runtime.rs:95-105` are six methods that pass straight
through to `RuntimeManager`; `ActorRuntime` repeats four of them
(`146-156`). `with_session` (`77-80`) is called at both call sites
immediately after `new` (`session.rs:86, 202`). Each delegator has one or
two callers, all in `channel.rs`.

## Why it matters

A reader follows `ChannelRuntime::security_manager()` to find that it is
`RuntimeManager::security_manager()`. Six times.

## Considerations

- `pub(crate) fn manager(&self) -> &RuntimeManager` instead of the six;
  `new(manager, commands, session)` instead of the builder; tests get
  `ChannelRuntime::detached(manager, commands)`. Two production and eight
  test callers.
- `ActorRuntime` stays narrow on purpose (issue
  [69](69-scoped-actor-runtime.md)): there the delegation is the point,
  because components must not reach the manager.

## Done when

`ChannelRuntime` has no method that only forwards to `RuntimeManager`.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.1.8).
