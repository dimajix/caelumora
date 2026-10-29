# 501 — Tests build a personality field by field

**Status:** in progress
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Tests of several crates build a personality of the system's by a literal of every field, each with a helper of its own or none, so that a new field of `Personality` changes every one of them.

## The problem

A test that needs a personality without anything particular writes a `Personality { … }` with
every field: in the prompt providers' tests of `caelumora-agents` (briefing, language,
participants), in `controller::agent`, `actor::kinds` and the store suite, among others - some as
a helper of their own (`plain()` in `controller::agent` and in the briefing provider's tests,
`personality()` in the store suite), some inline. Issue
[492](492-a-document-is-written-under-two-locks.md) added a field and changed every one of them;
one test grew past the length the lint allows and got a helper of its own for that alone.

## Why it matters

A field added to `Personality` is a change of a dozen test files that have nothing to do with it,
and the copies drift: each says what a "plain" personality is a little differently.

## Considerations

- **One helper in the model's testing.** A `testing` module of `caelumora_model::agent`, as
  `channel::testing` and `template::testing` are, gives a plain personality of the system's;
  tests write only what they differ in (`Personality { roles: …, ..plain_personality() }`).
- **A constructor of `Personality`.** If production code makes plain personalities too, the
  constructor is theirs; if only tests do, it belongs in `testing`.

## Done when

- A test that needs a personality states only what it differs in.

## History

- **2026-10-29** — Opened from the findings of the first step of issue
  [492](492-a-document-is-written-under-two-locks.md).
- **2026-10-29** — Decided for one helper in the model's testing, in two steps. A test gives a
  name - several store their personality, whose name is unique -, so the helper is
  `make_personality(name)`, as `make_template` and `make_channel` are named: a personality of the
  global scope, the system's, internal, naming nothing else. It is to live in `agent::testing`,
  brought back as `#[cfg(any(test, feature = "test-support"))] pub` for the tests of other crates.
  That module held `stored` and `replayed` so far, the model's own checks of any journaled state,
  of a channel's too, which belong to the stores. Step 1 done: `stored` and `replayed` moved to
  `store::testing`, for the model's own tests only, and `agent::testing` went. Step 2: the helper,
  and the tests that write every field of `Personality` use it.
