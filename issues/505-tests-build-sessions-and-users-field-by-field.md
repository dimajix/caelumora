# 505 — Tests build sessions and users field by field

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Tests write a `Session` or a `User` by a literal of every field - the model's, which cannot reach `session::testing::make_session` of the runtime, and those of every crate for a user, which has no `make_…` at all -, so that a new field of either changes tests that have nothing to do with it.

## The problem

Issue [501](501-tests-build-a-personality-field-by-field.md) gave the personality its
`make_personality` and wrote down the rule that a test starts from the `make_…` of its document's
kind and writes only what it differs in (CODING-GUIDELINES, *Documents in tests*). Two kinds of
the model are not there yet:

- **A session.** Its `make_…` is `session::testing::make_session(name, owner)`, in
  `caelumora-runtime`. The model's tests cannot reach it: the store suite writes every field of a
  `Session` in a `session()` of its own (`caelumora-model/src/store/suite.rs`), and about 26
  literals of `Session` stand in the workspace, some of them in the runtime's tests beside
  `make_session`.
- **A user.** `User` has neither a `make_…` nor a constructor: about 39 literals of it stand in the
  workspace, in the store suite (`user(username)`), the user providers, the controllers, the
  security suite and the prompt providers, among others. Some are production code: the user
  providers make the user they are asked for (`user_provider/accept_any.rs`, `file.rs`,
  `manager.rs`).

## Why it matters

A field added to `Session` or `User` changes dozens of tests, and the copies say each a little
differently what a plain session or user is. The rule of the coding guidelines now describes these
tests as not following it.

## Considerations

- **The document part of `make_session` moves to the model.** A `session::testing` of
  `caelumora_model` gives the session's document - `make_session(name, owner)` as it is now, read
  by its members (`Session::attributes_of`) -; the runtime's `session::testing` keeps what stores
  and starts it, and the store suite builds on the model's. Whether the runtime's tests import it
  from the model or the runtime re-exports it is to be decided with the step.
- **A user: a constructor or `make_user`.** The user providers make users in production, so a
  constructor `User::new(username, roles)` may be theirs, and tests use it with what they differ
  in; if the providers turn out to fill every field differently, `user::testing::make_user(name)`
  serves the tests alone, as `make_personality` does.
- **Sessions and users in one issue or two.** Both are the same problem and the same rule; they can
  be done as two steps.

## Done when

- A test that needs a session's or a user's document states only what it differs in.

## History

- **2026-10-29** — Opened from the findings of the review of issue
  [501](501-tests-build-a-personality-field-by-field.md), step 2.
- **2026-10-29** — Counted again: 9 tests write every field of a `Session` and 18 every field of a
  `User`; the numbers above counted other literals too. Decided in two steps. The model's
  `testing` modules build documents, the runtime's what runs and stores. Step 1: the documents of
  a session and its channels move from the runtime's `session::testing` to the model -
  `make_session` and `bare_session` to `session::testing`, `bare_channel`, `main_channel` and
  `founded` to `channel::testing` -, imported from there, not re-exported. Step 2: `User::new`,
  which `AcceptAnyUserProvider` makes its new users with, and `user::testing::make_user(name)` in
  place of the runtime's `session::testing::user`. Step 1 done: the 9 sessions are written by what
  they differ in, `make_session` on `bare_session`.
- **2026-10-29** — Step 2 done: `User::new(username)`, which `AcceptAnyUserProvider` makes its
  new users with, and `user::testing::make_user(name)` on it, in place of the runtime's
  `session::testing::user`; the 18 users are written by what they differ in. The file provider
  keeps its literal: it fills every field from its file, and a new field is to be decided there.
  `User::new` is `pub(crate)`: both its callers are the model's, and no provider lives elsewhere.
