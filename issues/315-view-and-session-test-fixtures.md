# 315 — The tests of the views and of sessions have fixtures of the same names

**Status:** open
**Area:** Code structure

## The problem

The tests in `view/mod.rs` and the fixtures in `session::testing` both build sessions and record
users, under the same names and with different signatures: `make_session(name, owner)` and
`make_session(name)`, `record_users(store, users)` and `record_users(manager, users)`.

Four tests about the views therefore live in `session/mod.rs` instead of with the views: that a
channel reads another by their classifications, that a reader knows the privacy of its session,
that a released message is read where its channel is not, and that a member's answer under way is
in the snapshot of its channel. Moved to `view/mod.rs`, their calls would reach the views' fixtures
instead of the session's. Two of them also build a `ChannelRuntime` from fields of `SessionLoop`
that are private to `session` (`commands`, `session_controller`).

## Why it matters

Whoever changes how a channel is read looks for its tests with the views and does not find four of
them. Two fixtures for one thing drift apart, and a test written next to the other set cannot be
moved without rewriting its setup.

## Considerations

- **One set of fixtures.** `session::testing` holds what builds a session for any test, and the
  views' tests use it; the views' own fixtures that do the same go. Their signatures differ - one
  takes an owner, one records users through the manager, the other through the store - so the
  callers of one of them change.
- **A `ChannelRuntime` of a running session for tests.** Instead of reading `SessionLoop`'s private
  fields, the two tests ask the session for it - a `#[cfg(test)]` method of `SessionLoop`, or a
  fixture in `session::testing` that is allowed to.
- Then the four tests move to `view/mod.rs`.

## Done when

- A fixture that builds a session or records users exists once.
- The tests of reading channels and of snapshots are with the views.

## History

- **2026-10-11** — Opened while splitting `session.rs`
  ([311](311-split-channel-and-session-loops.md)), which left these four tests in `session/mod.rs`.
