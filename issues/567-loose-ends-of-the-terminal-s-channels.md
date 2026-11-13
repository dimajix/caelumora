# 567 — Loose ends of the work on the terminal's channels

**Status:** in progress
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** The work on issue 561 left findings behind: tests that reach past the API they test or share their fixtures under `tests`, a stream of the REST server that misses a join taken back, an approval answered with its agent named by the caller, and a client that works out itself whom a question is put to.

## The problem

Issue [561](561-the-terminal-shows-one-channel-of-a-session.md) made the terminal follow every
channel of its session the user takes part in. On the way, the reviews of its steps found what is
not clean in the code around it. Those findings were reported and not fixed, and they are
collected here, to be taken on one after the other, each a step of its own:

1. **The REST server shares its test fixtures under `tests`.** `routes::tests`
   (`caelumora-rest-server/src/routes/mod.rs:389`) is `pub(crate)` and gives `login`, `state`,
   `state_with` and `read_until` to the tests of `agents.rs`, `attachments.rs`, `channels.rs` and
   `events.rs`. What the tests of several files use lives in a `testing` module (CODING-GUIDELINES,
   "Shared fixtures"): `routes::testing`.
2. **Three tests of the REST server learn alice's id the same way.** `stopped_session_of_alice`
   (`routes/channels.rs:972`) and the attachment test (`routes/attachments.rs:133`) ask
   `/auth/me` by hand, parse the answer into a `UserId` and make a session of hers with her in it;
   `routes/mod.rs:616` asks `/auth/me` as well. A shared fixture in `routes::testing` - the id of
   whoever a cookie logs in, a session of theirs - would hold it once.
3. **A test of the kernel reads a private field.**
   `an_actor_ended_before_it_ran_shows_its_end_alone` (`caelumora-kernel/src/actor/user.rs:1154`)
   reads `following.shown.try_recv()` to see that neither an envelope nor the start or end of a
   run was shown. Through the watch's API that is a `timeout` on `next()` that has to run out.
4. **`UserActor::watch` has no caller but tests.** The terminal was its last one; every frontend
   follows a channel through the views (`ChannelView::user_follow`), which checks that the channel
   lists the user. About fifty tests in the kernel, the agents, the skills, compaction and the
   tools still call it (`caelumora-kernel/src/actor/user.rs:809`). Either they follow through the
   views, or `watch` is offered under `test-support` alone, as `ChannelWatch::recv` and `try_recv`
   are since 561 - which closes a way of following a channel past the views.
5. **A loss on the bus cannot be caused outside the kernel.** `EventBus::publish_past_capacity`
   (`caelumora-kernel/src/event.rs:564`) is `pub(crate)` and used by the kernel's own tests. The
   terminal makes up for a loss (`Channels::follow_anew`), and its test
   (`following_anew_follows_only_what_is_not_followed`) can show only the making up, not that a
   loss leads to it. Offered under `test-support`, it would let a subscriber outside the kernel test
   what it does at a loss.
6. **The guidelines describe one of three ways a type offers methods to tests.** CODING-GUIDELINES
   ("Shared fixtures") describes what a `testing` module adds to a type, named apart from its own
   API. The kernel also has methods guarded by `#[cfg(any(test, feature = "test-support"))]` beside
   the type's own (`EventSubscription::recv`, `ChannelWatch::recv` and `try_recv`), and a separate
   `impl` block "past its own API" (`UserActor::take_in_as_routed`). Which of them are meant, and
   when each, is to be decided and written down - or the others aligned to the one described.
7. **A stream of the REST server misses a join taken back.** It reads whether the channel still
   lists its user where a run of their actor begins or ends (`ChannelFollower`,
   `routes/channels.rs`). A join is announced once it is stored and taken back with a leave where
   the newcomer's actor cannot start; a stream opened in between is never told, since no run ends,
   and stays open until its token expires. The terminal reads anew at a leave announced (561, step
   15); the stream holds no subscription of the bus (CHANNELS.md, "A user's live stream").
8. **An approval is answered with its agent named by the caller.** `UserActor::send_approval`
   (`caelumora-kernel/src/actor/user.rs:592`) takes the whole `ApprovalResponse`, the agent
   included, so that every frontend keeps the pending requests itself to find the agent: the
   terminal in `Followed::pending_approvals`, the REST API by `agent_id` in `SendApprovalRequest`
   (`caelumora-rest-api/src/channel.rs:433`), which the web UI sends. An answer to questions takes
   the id alone and the actor finds the asking agent (`answer_questions`); since 561 the actor also
   keeps the questions open for the user (`questions_to_answer`). The approvals the same way would
   leave one owner of what waits for the user.
9. **The web UI works out whom a question is put to.** `askedOf` in the web UI's
   `components/ChatView.tsx:1402` repeats `Questions::is_asked` of the domain
   (`question.rs:245`) in TypeScript. The snapshot could say which questions the user may answer.

## Considerations

- **Order.** The test fixtures of the REST server first (1, 2), then the kernel's tests and what it
  offers them (3 to 5), then the rule they follow (6) - written once the cases are cleaned up -,
  then the changes of behaviour and interface (7 to 9), each put before the maintainer with its
  interface first.
- **4:** following through the views asks every test for a `Views` of its user; `test-support`
  changes no test but leaves a second way in the tests. Which of the two is to be decided with the
  step.
- **7:** the stream could subscribe to the bus for its channel and read anew at a leave of its
  user, as the terminal does - a subscription per open stream -; or the kernel could end the
  newcomer's outbox where it takes a join back, so that every follower is told by its watch and
  nobody needs the bus for it. The second fits the rule that a stream ends because it is followed
  through its actor; it touches [564](564-taking-someone-in-is-rolled-back-in-part.md), which takes
  on the rollbacks of a join.
- **8:** `send_approval(id, approved)` changes a public interface of the kernel and the REST API,
  and with it the web UI; the terminal then asks the actor for what waits, as it does for the
  questions. Whether the approver needs the agent from the answer is to be checked first.
- **9:** whether a question may be answered is the actor's to say (8, and 561, step 11); after a
  user left and joined again, the new actor does not know the questions still waiting for them,
  which 561 left as it is (a corner case). Marking the snapshot follows the same source as the
  actor, or the two disagree.
- **Not part of it:** the terminal's `select_all` looks at the channels in the same order, so that a
  busy first channel can delay what another tells - judged harmless in 561; what a channel joined
  shows before the terminal follows it, and what the terminal tells at its start, are issues
  [565](565-the-terminal-misses-what-a-channel-joined-shows-at-first.md) and
  [566](566-the-terminal-tells-little-at-its-start-and-of-withdrawals.md).

## Done when

- Each of the points above is fixed as decided with the maintainer, or recorded under `## History`
  as decided against, with the reason.

## History

- **2026-11-13** — Opened from the findings of issue
  [561](561-the-terminal-shows-one-channel-of-a-session.md) that its steps did not take on, at the
  maintainer's request: one issue for all of them, taken on one after the other.

- **2026-11-13** — Step 1, point 1: the fixtures the tests of several route files share - `state`,
  `state_with`, `login`, `read_until` - live in `routes::testing`
  (`caelumora-rest-server/src/routes/testing.rs`), `pub(super)` and guarded by `#[cfg(test)]` like
  the crate-private `testing` modules elsewhere; `routes::tests` is private again and keeps what
  only its own tests use. The doc comment of `login`, which stood above `read_until` since 561,
  is back at `login`. No behaviour changes.

- **2026-11-13** — Decided with the maintainer on the review of step 1: three findings of it join
  the points above. To point 2: `send` in the tests of `routes/mod.rs` and `call` in those of
  `routes/agents.rs` are one helper written twice - a request with a JSON body as whoever a cookie
  logs in, answering the status and the JSON -, to be one in `routes::testing`. To point 6:
  `ApiError::status` and `code` (`caelumora-rest-server/src/error.rs`) are `#[cfg(test)] pub`
  beside the type's own API, not named apart from it, for the tests of `routes/channels.rs`; and
  the guidelines describe a crate-private `testing` module as `#[cfg(test)] pub(crate)` only, not
  as a private `#[cfg(test)] mod testing` with `pub(super)` items, as step 1 and five modules
  before it have it.

- **2026-11-13** — Step 2, point 2: `routes::testing` holds three more fixtures. `send` asks the
  API as whoever a cookie logs in - `Method` and an `Option` of a JSON body, the signature of the
  `call` of the agents' tests, which, like the `send` of `routes::tests`, is gone. `user_id` is the
  id `/auth/me` answers for a cookie, and `session_of` a session of a user, made as the system with
  them in its main channel, started. The attachment test and `stopped_session_of_alice` in
  `routes/channels.rs` build alice's session with them; the test of a project read by its owner's
  name in `routes/mod.rs` takes its ids from `user_id`. On the review, at the maintainer's
  decision, the requests the route tests still built by hand that `send` makes - a message posted
  in the attachment test and by `say`, `page_of`, the refusal of a stranger following a channel,
  the plugin's route asked after the login - go through `send` too, and `login` names the header
  by `CONTENT_TYPE`; the attachment test is short enough now to lose its `expect` of
  `clippy::too_many_lines` (issue [490](490-functions-longer-than-100-lines-since-rustfmt.md)). No
  behaviour changes.

- **2026-11-13** — Step 3, point 3: `an_actor_ended_before_it_ran_shows_its_end_alone` sees that
  nothing more is shown through the watch's API - a `timeout` of 100 ms on `next()` that runs out,
  as the tests of the locks and the approver wait for nothing to come - instead of reading the
  private `shown`. No other test reads a field of the watch. On the review, at the maintainer's
  decision: `send_message_asks_for_a_wakeup_and_returns_the_span_of_all_its_envelopes` read two
  private fields of the actor, its queue `user_rx` and its `channel_id`; it runs the actor now and
  reads what it forwards to the channel, waits for nothing else as above, and takes the channel's
  id from the channel of its fixture.
