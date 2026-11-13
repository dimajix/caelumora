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

- **2026-11-13** — Decided with the maintainer before step 4: `UserActor::watch` stays for the
  tests under `test-support`, rather than the tests following through the views - most of them
  test an actor or an agent, not who may follow, and a fixture of their own through the views
  would be a second way for tests all the same. Whether a method for tests may stand beside a
  type's own, as `watch` then does, is point 6's to decide, for it as for the others.

- **2026-11-13** — Step 4, point 4: `UserActor::watch` is guarded by
  `#[cfg(any(test, feature = "test-support"))]`, as `ChannelWatch::recv` and `try_recv` are since
  561, and its doc comment says why: whoever follows a channel does so through the views, as the
  user, where the channel lists them (`ChannelView::user_follow`). That any number may follow,
  and what is shown is dropped while nobody does, moved from its doc comment to `ChannelWatch`'s,
  since it holds of every follower. No test changes; the terminal, the REST server and the app
  build without it.

- **2026-11-13** — Step 5, point 5: `EventBus::publish_past_capacity` is `pub` under
  `test-support`, in its block of what the bus offers tests; EVENTS.md names it where it says how
  a subscriber's making up for a loss is tested. The terminal tests it with
  `a_loss_on_the_bus_follows_anew`: a channel is made with the user in it, the bus loses the
  announcement under more than a subscription holds, and the terminal follows the channel all the
  same. Without the arm of `taking_part` that follows anew at a loss, the test fails. On the
  review: the test waits for the bus through a subscription of its own, not the private one of
  `Channels`; and waiting until the user is told of a channel they take part in now, written out
  in three tests, is one helper, `until_told`.

- **2026-11-13** — Decided with the maintainer on the review of step 5: to point 6, the terminal's
  test fixture `settled` (`caelumora-tui/src/channels.rs`) reads the private `session_events` and
  `views` of `Channels`, since it has to take in what the bus delivered of the user's taking part
  through the subscription of `Channels` itself; a method of `Channels` doing so would be one for
  tests, and is decided with the others.

- **2026-11-13** — Decided with the maintainer before step 6: one form for what a type offers
  tests - an `impl` block of its own, guarded and saying what the tests do with the type past its
  API, in the `testing` module of the type's module or, where that cannot reach what it needs,
  right after the type's own `impl` -; the single methods guarded inside a type's own `impl` move
  into such blocks. The seven free functions for tests loose in a crate's `lib.rs` or beside the
  code - `prompts` and `catalogue` of four tool crates, the domain's `kernel_catalogue` and
  `meets_exactly_its_equals` - move into `testing` modules in the same step.

- **2026-11-13** — Step 6, point 6: CODING-GUIDELINES describes the three visibilities of a
  `testing` module - private with `pub(super)` items, `pub(crate)`, `pub` under `test-support` -,
  that a free function shared by tests is no exception, and the rule above under "What a type
  offers tests", with a line in the checklist. Moved into blocks of their own: `recv`, `try_recv`,
  `settled` and `drain` of `EventSubscription`; `recv` and `try_recv` of `ChannelWatch`;
  `UserActor::watch` into the actor's block; `ChannelLoop::observe` into `channel::testing`;
  `new_participant_runtime`, `transcript_store` and `prompts` into the block of `ChannelRuntime`;
  `ParticipantRuntime::acting_as`; `SessionManager::create`; `ResolvedParticipant::approver`;
  `ApiError::status` and `code`; `Turn::begun` and `under_way`; the store's `Connection::schema`;
  the web client's `direct` and `reaching_the_host`; `AgentChannelState::context_of`. The
  terminal's fixture `settled` is a method of a `#[cfg(test)]` block of `Channels`, so that no test
  reads its fields. The free functions live in `prompt_catalogue::testing` and `query::testing` of
  the domain and in a `testing` module of each tool crate; the doc comment of the workspace
  tools' `prompts`, which called the texts a catalogue, says what they are. No behaviour changes.
  On the review: every block for tests says what the tests do with the type past its own API,
  also the four that said nothing before (`EventBus`, `AgentRuntime`, `Policies` of the security
  crate, `AgentBody`); `nowhere`, which the actor's block and its tests share, lives in a private
  `testing` module of `actor::user`; and the guidelines say that a block for tests is one of what
  a test may use beside a `testing` module.

- **2026-11-13** — Decided with the maintainer on the review of step 6: a block for tests lies in
  the nearest `testing` module above the type that reaches what its methods need, the crate's own
  included, and only where none does beside the type; the two findings of the review join the
  step. So `AgentChannelState::context_of` moved to the domain's `agent::testing`,
  `ResolvedParticipant::approver` to `channel::testing`, and `Turn::begun` and `under_way` to
  `caelumora_agents::testing`, beside its other block for `Turn`. The tests of the session manager
  ask `running` instead of reading its map of sessions, and the tests of a running channel read
  the history it holds through `ChannelLoop::reading_history` of `channel::testing` instead of its
  state. The crate-root `testing` modules of `caelumora-model` and `caelumora-compaction` give
  their items as `pub(super)`, as the guidelines describe a private one.

- **2026-11-13** — Decided with the maintainer before step 7: of the three options - the kernel
  showing an end where it takes a join back, the stream subscribing to the bus, or leaving it -
  the first, so that every follower ends through its watch, as at the end of any run.

- **2026-11-13** — Decided anew with the maintainer on the review of that change: its premise does
  not hold for users. A user's actor is never refused as it is built (`UserActor::new` has no step
  that fails), so the rollback of an addition does not reach a user but in the race of two takings
  in at once that issue [564](564-taking-someone-in-is-rolled-back-in-part.md) takes on - and only
  a user is followed by a frontend. The point as this issue states it came of a wrong reading in
  561. The change was dropped before it was committed. The gap the review found instead is taken
  on: a frontend followed a channel by the listing of the view it had read, and subscribed to the
  outbox only after it - one who left in between was followed past the end their actor showed
  before the watch began, a stream of the REST server until its token expired.

- **2026-11-13** — Step 7, point 7 as decided anew: `ChannelView::user_follow` begins the watch
  first and then reads whether the channel lists the viewer as it stands
  (`SessionView::document_now`): through the session's loop if it runs now - not the loop the view
  reached, which a stop or a deletion leaves behind -, else from the store, and as none once the
  session is deleted, which hides its channels with it; nothing is asked of the policy again. So a
  follower who left, or whose session went, before the watch began is not followed past the end
  their actor showed. It is `async` now and passes on a store that cannot be read
  (`ViewResult<Option<ChannelWatch>>`), which the REST server answers as a server error rather than
  as one who takes no part; the terminal and the security suite await it. Reading a channel of a
  stopped session that its store no longer has is now no channel rather than a failure, for every
  reader of `SessionView::document`. CHANNELS.md says so where it describes a user's live stream,
  and no longer gives the join taken back as a case no run ends, nor does EVENTS.md or the
  terminal's `Happened::TakingPart`. A join taken back in the race issue 564 takes on still ends no
  run; that is 564's to record when it is worked on.

- **2026-11-13** — Decided with the maintainer on the review of step 7, and done in it: the
  terminal no longer reads anew at a leave announced on the bus. Neither reason given for it in
  561 holds: a user's join is not taken back, and following now reads the channel as it stands once
  the watch began, so that the end of the user's actor's run tells every leave. Its own join, a
  channel restored and a loss on the bus are what it follows anew at (`changes_taking_part`);
  EVENTS.md says that the terminal learns of a leave from the end of the run, and CHANNELS.md no
  longer names the terminal's reading at a leave.

- **2026-11-13** — Decided with the maintainer before step 8, after a first attempt that looked
  the agent up in the agents' stored turns was dropped uncommitted: actors communicate through the
  channel alone, never by reaching into each other. A user's answer to an approval names the
  request alone - `ActorEvent::ApprovalAnswered { id, approved, decision }`, apart from the
  approver's decision (`ApprovalGranted`, `ApprovalDenied`, `ApprovalTimedOut`) -; the user's actor
  sends it through the channel, which no longer asks `ApproveToolCall` for it; the approver, which
  holds the request, resolves the agent, asks whether the user may approve its calls and counts the
  answer. An answer it does not count it tells the user of with a new event every actor may send
  back to whom an event of theirs went to and was not taken: `MessageRefused`, beside the channel's
  `MessageRejected`, with a history entry of its own, so that no stored entry changes. The
  frontends follow in a step of their own.

- **2026-11-13** — A point joins the issue, from a review of the code at the maintainer's request:
  10. **Actors reach into each other.** The user's actor checks before it sends whether the agent
  it asks to answer again exists and whether its turn is under way (`UserActor::request_retry`,
  `request_retry_of_turn`, through `ChannelLoop::agent_actor` and `retryable_turn`), and reads the
  state of the participant it nudges (`request_response`, `actor.state().in_turn()`); the approver
  reads the security context of the users' actors (`approvers_of`) and whether an agent's actor
  runs (`catch_up`). Actors communicate through the channel alone: the checks go to the actor
  asked, which refuses what it does not take (`MessageRefused`), and what the approver needs it
  takes from the channel's participants. The approver's reading of the users' contexts goes with
  step 8.

- **2026-11-13** — Step 8, point 8 in the kernel, as decided: a user's answer is
  `ActorEvent::ApprovalAnswered { id, approved, decision }`, recorded as
  `HistoryEntry::ApprovalAnswered` to the approvers; `ApprovalGranted`, `ApprovalDenied` and
  `ApprovalTimedOut` - `HistoryEntry::ApprovalDecided` - are the approver's decision alone.
  `UserActor::send_approval(id, approved)` and an answer by a rule of the user's send it. The
  channel lets it in from its user with their own part alone and asks nothing more of it; the
  approver counts it if it holds the request and the user may approve the calls of the agent that
  asked (`ApproverActor::handle_answer`), asked in the context the channel admits the user in
  (`ChannelLoop::context_of`) - as `approvers_of` now asks too, no longer reading the users'
  actors -, and refuses any other with `MessageRefused` to the user alone: `Invalid` for a request
  it does not hold, `Forbidden` for one who may not approve. `MessageRefused { to, refused, message,
  reason }`, with `HistoryEntry::MessageRefused`, is what any actor sends back to whom an event of
  theirs went to that it does not take; the channel lets it in to someone else taking part, and
  the REST API's history gives it as an item. The terminal and the REST server pass the id alone;
  the rest of the frontends is step 9. APPROVALS.md, CHANNELS.md, PERMISSION-CHECKS.md and
  SECURITY.md say so. No stored data has to be deleted: an answer a user gave before is kept as an
  `ApprovalDecided` of theirs, which still goes to the approvers as it was recorded, who no longer
  take it. On the review: the approver enforces `ApproveToolCall` with `Enforcement::require`, as
  the owner of the act (SECURITY.md rule 4, PERMISSION-CHECKS.md), and asks `may_approve` only to
  know whom to wait for; an agent it cannot read is refused as `Invalid`, as the channel answered
  it before. A refusal is made of the envelope refused (`Envelope::refusal`), naming its kind and
  its message, for every actor that will send one. The doc comments of `ApprovalResponse` and
  `Rejection` say what they are now, the history's fixture covers the two new events, the channel's
  admission of both is tested, and the terminal tells a refusal (`[refused] … did not take your
  …`), so that a refused `/approve` is not silent before step 9; REST-SERVER.md names it among what
  a stream carries.

- **2026-11-13** — Decided with the maintainer on the review of step 8, and done in it:
  `ApprovalResponse`, which now carries the approver's decision alone, is renamed
  `ApprovalVerdict`, in the code and in the web UI's types; its fields, and so what is stored, stay
  as they are. Any participant may send `MessageRefused` to any other, as the channel admits it:
  the scope intended. The approver's tests read its private `pending`, a finding that joins
  point 10.

- **2026-11-13** — Step 9, point 8 in the frontends: `SendApprovalRequest` loses `agent_id`, and
  the web UI's `respondToApproval` sends the request's id alone. The web UI tells a refusal in a
  notice (`MessageRefused`, live and from the history's `message_refused`), as it tells a
  rejection, and knows an answer of the user's rule from their `ApprovalAnswered`; its types name
  both events. The terminal keeps the requests shown until the approver ends them, to find the
  channel `/approve` answers in and to show them again at `/switch`, and `/approve` sends through
  the user's actor directly (`Followed::approve` is gone). WEB-UI.md and REST-SERVER.md say so.
  On the review: a request the user answered - here, on another page of theirs or by a rule of
  theirs, as their actor shows them their `ApprovalAnswered` - is theirs to answer no longer, in
  the terminal (taken out of the requests shown) as on the web UI (marked answered), so that
  neither offers a second answer. The terminal keeps the requests shown, rather than asking the
  user's actor as for the questions, as point 8 considered: since step 8 no actor of the user's
  holds approvals - the approver does -, and what the terminal keeps is what the stream showed it,
  as the web UI keeps it. A refusal names the approver also when read back from the history, and
  `invalid` is worded so that it fits a request that is over. The REST handler's doc comment says
  how an answer goes.

- **2026-11-13** — Decided with the maintainer on the review of step 9, and done in it:
  `MessageRefused` is renamed `EventRefused` - what an actor does not take is an event of any
  kind, and not every one a request: an answer is none -, with its history entry, the REST API's
  item (`event_refused`), the web UI's types and the documents; no stored data holds it but on this
  branch. A refusal names the answer it refuses as its correlation (`EventContext::correlation_id`,
  the answer's `event_id`), and every page of the user sees their own `ApprovalAnswered`: the web
  UI remembers which request each answer was to, and where a refusal of it comes, marks the request
  answered no longer - without a change of the REST interface.

- **2026-11-13** — A point joins the issue, at the maintainer's decision:
  11. **`MessageRejected` names an event as a message.** What the channel does not let in is an
  event of any kind (`rejected` names it), as what an actor does not take is (`EventRefused`): it is
  to be renamed `EventRejected`, with `HistoryEntry::MessageRejected`, the REST API's
  `message_rejected` and the web UI's notice - which changes stored history entries, so that the
  stored histories have to be deleted.

- **2026-11-13** — Point 9 decided against with the maintainer: `askedOf` in the web UI reads
  which actors the questions are put to from what it was sent - as `Questions::asked` and
  `is_asked` read the same field -, and decides nothing: whether the user may answer the user's
  actor checks before it sends, and the asking agent as it takes the answer. A mark in the snapshot
  would serve a page as it loads alone, since `QuestionRequested` comes to every receiver alike,
  and leave two ways for one thing. What stays open is the source of the open questions after a
  user left and joined again, which 561 left as it is.

- **2026-11-13** — Decided with the maintainer before steps 10 and 11, for point 10: whether an
  agent asked to respond is answering already is the agent's to tell, not a guard's. A request to
  respond that is the very next event in an agent's inbox as its turn ends - nothing unheard ahead
  of it, so that the conversation stands as the turn answered it - is answered by that turn and
  dropped; the user's actor no longer reads the state of whom it nudges, and `AlreadyResponding`
  goes (step 10). The other checks go to the channel's door, which holds its actors and decides
  what enters, rather than to the actor asked: a request to answer a message again is admitted
  there already; whether a turn can be answered again (`retryable_turn`) is admitted there too,
  refused with `MessageRejected` instead of dropped unsaid; and the approver asks the channel
  whether an agent's actor runs (`ChannelLoop::runs`) instead of reaching for it, and its tests
  watch what it sends instead of reading `pending` (step 11).

- **2026-11-13** — Step 10, point 10 for the nudge: an agent drops a request to respond that is
  the very next event in its inbox as a turn ends as it should, in the turn's authority
  (`AgentBody::drop_answered`, called by the run loop after a step that reflected on the turn) -
  as it drops one heard during the turn. `UserActor::request_response` checks no more than that the
  user does not nudge themselves - whether whom they ask takes part the channel's door admits, as
  it did - and `ChannelError::AlreadyResponding`, with its REST code `already_responding`, is gone;
  `/participant nudge` asks an agent in a turn as any. AGENT-TURN.md and CHANNELS.md say so. On
  the review: the drop happens at the end of `handle_reflecting`, once the turn is taken over as it
  should - not after a failed one -, rather than in the run loop; an external agent drops so as its
  answer is published, which the guard had protected too; who began the chain an envelope was sent
  in is asked of one place (`Envelope::in_authority_of`), also by the personality's picks; the drop
  is logged in the request's span; and a test runs the agent end to end: a nudge while it answers
  begins no turn of its own.

- **2026-11-13** — Step 11, the rest of point 10: the user's actor no longer checks whether the
  agent it asks to answer again takes part, nor whether its turn can be answered again
  (`request_retry`, `request_retry_of_turn`); the channel's door decides both - it admitted only a
  message an agent taking part said already, and now admits a turn only if it can be answered
  again (`retryable_turn`), refusing any other with `MessageRejected` instead of dropping it unsaid
  as it is delivered - but for one whose turn ended between its admission and its delivery, which
  is dropped as before. `ChannelError::NotAnAgentsMessage` and `TurnNotRetryable`, with their REST
  codes, are gone. The approver asks the channel whether an agent's actor runs
  (`ChannelLoop::runs`) instead of being handed it, and its tests see that it holds a request no
  longer by the refusal of an answer to it rather than by reading `pending`. CHANNELS.md and
  PERMISSION-CHECKS.md say so.
  On the review: `agent_actor` and `retryable_turn` are private to the channel now; the channel's
  door asks `runs` too where it asked for an actor only to see whether it runs; the REST handler
  of a turn's retry says that a refusal comes on the stream; the agents' security suite describes
  what it shows; a test of the kernel shows a turn that cannot be answered again refused at the
  door; and CHANNELS.md states the rule that actors reach each other through the channel alone.

- **2026-11-13** — Decided with the maintainer on the review of step 11, and done in it: a request
  to answer again no longer sends a `WakeupRequested` ahead - which wakes every actor of the
  channel, and now also for a request the door refuses -; it is deliberate as a nudge is, so that
  the channel ends being quiet as it takes it in and wakes the agent it goes to alone
  (`send_retry`, `ChannelLoop::enter`). CHANNELS.md says so where it tells of waking and of asking
  for another answer. The user's actor still reading a message to change it before the door does
  (`message_to_change`), and resolving whom a command names by the commands other agents hold
  (`request_command`), stay findings for the maintainer.
