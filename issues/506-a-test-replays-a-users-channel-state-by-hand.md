# 506 — A test replays a user's channel state by hand

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `what_was_read_only_moves_forward` replays the changes of a `UserChannelState` by hand and checks only `read`, where the tests of every other journaled state use `store::testing::replayed`, which also checks that replaying is idempotent and records nothing.

## The problem

Four kinds of state are journaled (`Journaled`): `AgentState`, `AgentChannelState`, `ChannelState`
and `UserChannelState`. The tests of the first three check their changes by
`store::testing::stored` and `replayed`: the state as a store holds it, its changes replayed onto
it twice, the copy the same as the state, and nothing recorded by replaying. The test of the
fourth, `what_was_read_only_moves_forward` (`caelumora-model/src/user.rs`), applies the changes
once to a default by a loop of its own and compares only the read mark.

## Why it matters

A check of the same thing is written twice, and the copy checks less: a change of a user's channel
state that does not replay idempotently, or that records something when it is replayed, passes it.

## Considerations

- **Use `replayed`.** `replayed(&state, &mut UserChannelState::default())`, then the test's own
  assertion of the read mark; `stored` if the test begins from a stored state as the others do.

## Done when

- The test of `UserChannelState` checks its changes by `replayed`, as the others do.

## History

- **2026-10-29** — Opened from the findings of the review of issue
  [501](501-tests-build-a-personality-field-by-field.md), step 1.
