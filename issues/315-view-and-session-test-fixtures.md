# 315 — The tests of the views and of sessions have fixtures of the same names

**Status:** done
**Area:** Code structure
**Summary:** The tests of the views and of sessions have fixtures of the same names and different signatures, so four tests of the views live with the sessions.

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
- **2026-10-11** — Done, in tests only. `session::testing` holds one fixture of each kind, and the
  views', the sessions' and the channels' tests use it:
  - `make_session(name, owner)` and `main_channel` in the shape of the views' fixtures, which was
    the production one (`Template::build`, `Channel::build`): read by the group of the session's
    or the channel's participants, and the main channel takes the session's participants. The
    sessions' version had neither; two channel tests relied on that and now say what they need -
    one makes its reader a participant instead of copying a grant of the session into the
    channel, the other gives its main channel no participants.
  - `user(name)`, which replaced the views' fixture, the one in `channel/tests` and the literal
    `User { … }` in the tests of sessions, channels and views.
  - `record_users` through the manager; the views' tests have one at hand.
  - `owned_runtime(rules)`, which replaced the views' two fixtures and twelve copies of the same
    four lines in the tests of sessions and channels; `runtime_of(manager)` for the tests that
    ask the runtime directly.
  - `views_in(session, channel, actor)`, which reads `SessionLoop`'s private fields from inside
    `session`, so `SessionLoop` needs no method for tests.
  - `bare_session`, moved from `runtime.rs`, so that every fixture building a session is here.

  The four tests moved to `view/mod.rs`. Left as they were: the literal `User { … }` in tests
  outside sessions, channels and views; `views_in` in the tests of `agent_memory`, which builds on
  `ViewsFactory::in_session` and belongs to [309](309-options-none-only-in-tests.md).
