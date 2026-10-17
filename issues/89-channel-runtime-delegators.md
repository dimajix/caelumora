# 89 — `ChannelRuntime` is a layer of delegators without a life of its own

**Status:** rejected
**Area:** Code structure
**Epic:** [74](74-refactoring-epic.md)
**Summary:** `ChannelRuntime` is six methods that forward to `RuntimeManager`.

## The problem

`caelumora-runtime/src/runtime.rs:95-105` are six methods that pass straight through to
`RuntimeManager`; `ActorRuntime` repeats four of them (`146-156`). `with_session` (`77-80`) is
called at both call sites immediately after `new` (`session.rs:86, 202`). Each delegator has one or
two callers, all in `channel.rs`.

## Why it matters

A reader follows `ChannelRuntime::security_manager()` to find that it is
`RuntimeManager::security_manager()`. Six times.

## Considerations

- `pub(crate) fn manager(&self) -> &RuntimeManager` instead of the six; `new(manager, commands,
  session)` instead of the builder; tests get `ChannelRuntime::detached(manager, commands)`. Two
  production and eight test callers.
- `ActorRuntime` stays narrow on purpose (issue [69](69-scoped-actor-runtime.md)): there the
  delegation is the point, because components must not reach the manager.

## Done when

`ChannelRuntime` has no method that only forwards to `RuntimeManager`.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 3.1.8).
- **2026-09-29** — Rejected with the maintainer. Part of the problem is gone already: `with_session`
  and the builder no longer exist, `ChannelRuntime::new(manager, commands)` is the whole
  construction, so no `detached` is needed. What remains are the six one-line delegators
  (`runtime.rs:83-94`) with six callers, all in `channel.rs`. Replacing them by `manager()` would
  turn every call into `runtime.manager().x()` - longer at each call site for six lines saved in
  `runtime.rs`; the gain in readability is too small to justify it. `Deref<Target = RuntimeManager>`
  hides the same delegation and was not considered further; letting `SessionLoop` build moderator,
  recorder and controller for `ChannelLoop::new` moves knowledge about what a channel needs to the
  session. `ChannelRuntime::event_bus()` has no caller; that belongs to
  [107](107-public-interface.md).
