# 235 — An answer still being generated cannot be asked for again

**Status:** done
**Area:** Sessions and channels
**Summary:** An answer still being generated cannot be asked for again; only stopping every agent of the channel gets there.

## The problem

A user can ask an agent to answer again only once its answer is complete
([234](234-modify-revoke-and-retry-messages.md)): the web UI offers the retry icon on a stored
message while nothing is being generated, and an agent reads its inbox only between turns, so a
request sent during a turn waits for the turn to end. An answer that is visibly going wrong - a
derailed plan, a tool loop - has to be stopped first with the channel's stop button, which ends the
turns of *every* agent in the channel, and the stopped answer is then asked for again.

## Why it matters

The answer that goes wrong while one watches is the one most often wanted again. Stopping the whole
channel to retry one agent's answer is a detour, and in a channel with several agents answering at
once it stops answers nobody wanted stopped.

## Considerations

- The maintainer's wish when 234 was planned: retry only a completed single message, or - if it is
  not completed - cancel the current turn first.
- `ChannelLoop::cancel` acts on the channel as a whole (`CancelChannel`); cancelling one actor
  (`Actor::cancel`) is crate-internal and checks nothing, as it is the consequence of an act that
  was checked (SECURITY.md). A retry of an answer under way needs an operation that cancels the
  author's turn as part of asking it to answer again, asked as `RetryMessage` - not a way around
  the channel's cancel.
- The message being streamed has an id (`PartialMessageGenerated` carries one); whether it is the
  id the complete message will have, and so one a request can name, has to be checked. A cancelled
  turn is taken over, so its partial answer becomes a message of the history - which the existing
  retry then replaces.
- What the user sees in between: the cancelled answer appears, then is taken back. Acceptable, or
  should the UI hide it?

## Plan

Written against the code as of `64ca713`, with the maintainer.

### What the code shows

- **An agent reads its inbox between turns only** (`handle_waiting`, `actor_impl/agent.rs`).
  While a turn runs, the only way to reach it is its `RunControl` (`agent_run_control.rs`):
  synchronous, an `Interrupt::Cancel` or `Interrupt::Stop` taken after every step, and a
  `CancellationToken` the streaming loop, a running tool and a wait for an approval all wait on.
  Whatever asks for an answer under way to end has to go that way.
- **A streamed partial message has no id a request could name.** Every chunk gets a new
  `MessageId`, and the complete message yet another (`ollama_client.rs`); the web UI gathers the
  answer under way per sender, not by id. This answers the question in *Considerations*: no. What
  a request can name is the turn.
- **Nothing of a turn under way has reached anybody but the users.** `CompleteMessageGenerated`
  and the partial events go to the users; the channel's history and the other agents learn of a
  turn with its `FullResponseGenerated` alone, which `Turn::commit` sends once the turn is taken
  over (`agent_turn/ending.rs`). A turn given up (`give_up`, issue 145) sends nothing of what it
  produced.
- **A cancel does not get there.** `commit(cancelled)` takes the complete messages over, tool
  messages included, and `AgentChannelState::retried` then refuses exactly the answers this issue
  is about: a tool loop ends in a tool message (`NotLast`), a derailed plan in internal steps
  whose calls ran (`CallsRan`), a turn stopped in its first model call in no message at all.
  "Cancel first, then retry" cannot be built on the retry of a completed message.

### What was weighed

- **The client combines a cancel of the one actor with the existing retry.** Cheapest; but the
  rule refuses (above), the stopped answer stands in the history and with every agent until it is
  taken back - and may be answered meanwhile - and two requests with a race between them have to
  find the message the cancel produced. Not chosen.
- **The retry gives the turn under way up** (the mechanism of issue 145). Chosen: nothing of the
  turn reached the history or the other agents, so nothing has to be taken back and nobody can
  have answered it; the calls that ran are told to the model as a given-up turn tells them; the
  objection 234 raised against moving the author's heads - its stores would differ from everyone
  else's - does not apply, since nobody else holds anything of a turn under way.
- **The agent reads its inbox during a turn.** Breaks "what is heard is taken in between turns"
  and everything `take_in` and `commit` rest on. Not chosen.
- **Where the turn's id lives.** Only in `RunControl` and `ResponseStarted`: enough for the
  interrupt, but a client that loads the page during a turn, and the record of which messages
  belong to which turn, have nothing. On every message as well (chosen): the record is explicit
  instead of counted in `ContextRef::turn_entries`, a reload finds the turn through any message of
  it, and the UI can take exactly the messages of a given-up turn off the screen. In
  `ResponseStarted` too (chosen): not every agent streams, and the answer of one that does not has
  no message before it ends.
- **How the request names its target.** A second event beside `RetryRequested`, or one event with
  a target that is a message or a turn (chosen): one concept, one routing, one permission, one
  waking; a completed message and an answer under way are different targets with different
  consequences, which the target says.
- **Cancelling one actor** (`ChannelLoop::cancel_actor`, the maintainer's proposal) is an
  operation of its own and the answer to "stops answers nobody wanted stopped"; it is not a
  building block of the retry, for the reasons of the first option. Built here, after
  `wakeup_actor`, under the same permission as `cancel`.

### The rules

- **Only the turn the request names is interrupted.** Whether a retry of an answer under way is
  possible is decided before the interrupt, from what is reachable synchronously: is the named
  turn the one under way? If it is, the retry is always possible - the agent gives its own turn
  up. If it is not - ended meanwhile, or never this agent's - nothing is interrupted: a retry
  asked of an older message while a new answer is being generated does not stop that answer. The
  retry of a *completed message* never interrupts anything; it waits in the inbox and is judged
  after the turn, as today.
- **A retry that hit its turn is a fact, not a candidate.** Once the turn is given up for it,
  `take_in` cannot reject it any more - the answer is gone - so it is accepted whatever else is in
  the batch: a `ResponseRequested` that came in meanwhile goes into the same turn, which is what
  the agent would do after the turn anyway, with the derailed answer before it. The one turn runs
  with the authority of the last who asked, as `take_in`'s rule says.
- **A retry of a completed message with a request in the same batch** is judged against the
  conversation as it stands, not refused unseen as 234 built it: if it can be done, what it takes
  back is taken back and the one turn answers both (a request without a message - an agent chain,
  a wakeup - would otherwise produce a second answer after the one that was to be replaced); if it
  cannot, it is rejected with its actual reason - usually `NotLast` - and the request is
  answered. `AskedAgain` stays for the earlier retries of a batch.
- **What is told, and when.** The user's actor asks before it sends, as for every change (234),
  so that a retry that cannot hit its turn is refused at once, synchronously, and no event is
  sent. The channel is the gate: it admits the request only with the permission and interrupts
  the turn before it delivers the request, as it wakes an agent before it delivers; a request
  whose turn ended between the two checks is dropped and logged, like any change the channel does
  not admit - no `RetryRejected` for it: the users see the turn end, and the retry icon on the
  completed message. For an answer under way there is thus no late `RetryRejected`, with one
  exception: a cancel that was asked first wins, the turn is taken over, and the request then
  finds no turn given up for it - `RetryRefused::Ended`.
- **Interrupts:** `stop` wins over everything; between `cancel` and `retry` the first wins
  (`compare_exchange` from no interrupt, as `cancel` does today).

### The model (`caelumora-model`)

- **`TurnId`**, after `MessageId`: a `Uuid`, `Display`, serialized as a string.
- **`Work::turn_id: TurnId`**, given when the work begins (`Work::new`), stored with it, so that a
  turn resumed after a restart keeps its id; `#[serde(default)]` to a new one for work stored
  before. `Work` is also what an external agent's answer or task runs as; it gets an id alike, so
  that `None` on a message means one thing.
- **`Message::turn_id: Option<TurnId>`**: the turn that produced it; `None` for a user's message,
  the runtime's outside a turn, and messages stored before. Not covered by any digest
  (`digest::message_chain` names its fields; this one is not added, like `timestamp`), so every
  digest recorded before keeps naming what it named. Set where a turn makes a message its own -
  `stamp` for what the agent's model says, partial and complete, and the external agent's
  equivalent; `TurnState::push` sets it for everything the transcript keeps (the tool message of
  `ToolCalls::collect`, the message a given-up turn tells its model with), so that the invariant
  holds without touching every builder. `Message::modify` does not take it over, as it does not
  take tool calls over: a changed message stays in the turn it was produced in.
- **`RetryTarget`**, what a retry is of, in `actor.rs` next to the events:

  ```rust
  #[serde(rename_all = "snake_case")]
  pub enum RetryTarget {
      /// A message the agent said, in the channel's history.
      Message(MessageId),
      /// The answer the agent is giving in this turn, still under way.
      Turn(TurnId),
  }
  ```

  `RetryRequested { target, author }`, `RetryAccepted { target }`, `RetryRejected { target,
  reason }` replace their `message`. `ResponseStarted { turn_id }`.
- **`RetryRefused::Ended`** ("its answer had ended"): the turn named was not given up for the
  request - a cancel won.
- **`GivenUpReason::Retried`**: the turn was given up because its answer was asked for again.
  `told_of_executed` words it for the model ("the answer was asked for again while these tools
  ran"); nothing is told to the users, `RetryAccepted` follows.
- **`AgentChannelState::retried(target, own)`** takes the target: for a message as today; a turn
  is not decided here - the agent's run loop knows which turn it gave up for a retry (below).

### The runtime (`caelumora-runtime`)

- **`RunControl`** keeps the id of the turn under way: `begin_turn(turn_id)`, and from the state
  it is built from, for a resumed turn. `turn_under_way() -> Option<TurnId>` while in a turn.
  `retry(turn) -> bool`: in one step under the lock, compares `turn` with the turn under way,
  leaves `Interrupt::Retry` if no interrupt was asked yet, cancels the token, and returns whether
  it did. `Interrupt::Retry` sits between `Cancel` and `Stop`.
- **`Actor`** gains `turn_under_way(&self) -> Option<TurnId>` (default `None`) and
  `retry_turn(&self, turn: TurnId) -> bool` (default `false`), synchronous like `cancel` and
  `wakeup`, and like them the consequence of an act that was checked: the channel calls them.
  `AgentActor` answers from its `RunControl`; the external agent keeps the defaults: its remote
  side keeps what it was given (`Elsewhere`), so no turn of it is under way to be asked again.
- **The run loop** takes `Interrupt::Retry` after the step it cut short, as it takes `Stop`:
  gives the turn up (`give_up` with `Retried`), settles idle, and goes on - to `handle_waiting`,
  which finds the request in the inbox. The id of the turn given up for a retry is a local of the
  loop, handed to `handle_waiting`; nothing of it is stored: a crash in between loses the request
  as it loses every event in flight, and the turn is recorded as given up either way.
  `give_up` is restructured so that the approval requests of the turn are withdrawn whenever the
  agent stays in the channel - today the `Stopped` branch returns before withdrawing, which is
  right for an agent that leaves and wrong for one that answers again - and `TurnFailed` is sent
  only where there is something to tell.
- **`handle_waiting` / `take_in`**: `take_in` returns the requests as today. `handle_waiting`
  takes the retry whose target is the turn given up as accepted - `RetryAccepted`, nothing to
  take back - and the last of the others as the candidate, judged by `retried` (a `Turn` target
  that is not the turn given up is `Ended`); an accepted or possible retry is carried out and the
  one turn begins in the contexts of the request if there is one, else of the retry; a refused
  one is rejected with its reason, and the request, if any, answered. The rest are `AskedAgain`.
- **`ChannelLoop::cancel_actor(context, actor)`**, after `wakeup_actor`: `CancelChannel` with
  the channel's attributes, then `Actor::cancel` of that one actor; an error if it is not in the
  channel. It does not make the channel quiet (see open question 1), so the taken-over answer
  reaches everyone as a `FullResponseGenerated` does and the moderator may ask another agent to
  respond to it.
- **The channel's gate** (`admits_change`), for a `RetryRequested` with a `Turn` target:
  `may_retry_message`, `author` an agent of the channel, and `turn_under_way() == Some(turn)`.
  Before delivering, where it wakes whom a request asks (`route_event_impl`), it calls
  `retry_turn(turn)` on the author; `false` drops that delivery with a warning. The routing is
  the moderator's as for every `RetryRequested`: to its author alone.
- **`UserActor::request_retry_of_turn(author: ActorId, turn: TurnId) -> Result<Uuid>`**,
  beside `request_retry(message)`: `RetryMessage` with `Enforcement::require`, `author` an agent
  of the channel (`NotAParticipant` else), `turn_under_way() == Some(turn)` through the channel
  (`ChannelError::NotUnderWay { turn, channel }` else), then `WakeupRequested` and the request,
  as `request_retry` sends them. Returns the `span_id`.

### The flow

1. The user sees an answer going wrong and asks for another, naming the agent and the turn the
   UI learned from `ResponseStarted` (or from any message of the turn after a reload).
2. The user's actor checks permission and whether that turn is under way, and refuses
   synchronously if not; else it sends the request.
3. The channel admits it under the same conditions, interrupts the turn (`retry_turn`), and
   delivers the request to the agent's inbox - or drops it if the turn ended in between.
4. The run loop ends the step under way at once - the model call is dropped, a running tool
   interrupted, a wait for an approval ended - gives the turn up: its messages become an inactive
   branch, its calls that ran are told on the active path, its approval requests withdrawn,
   nothing is sent to the channel.
5. `handle_waiting` takes in what came meanwhile, finds the request, answers `RetryAccepted` and
   begins the new turn, merged with a `ResponseRequested` of the batch if there is one, given the
   transcript as it stands: everything before the given-up turn, and what was heard since.
6. The users see `RetryAccepted` - the UI takes the messages of the given-up turn off the screen
   and shows the agent as answering - then the new answer as any answer.

### REST (`caelumora-rest-server`)

| Route | Does | Answers |
|---|---|---|
| `POST /sessions/{id}/channels/{cid}/participants/{aid}/cancel` | `ChannelLoop::cancel_actor` | `200`; `403`, `404` if no such participant |
| `POST /sessions/{id}/channels/{cid}/participants/{aid}/retry` `{ turn_id }` | `UserActor::request_retry_of_turn` | `200`; `403`; `409 not_under_way`; `404` if no such participant |

- `POST …/messages/{mid}/retry` stays as it is; `can_retry` keeps its meaning and covers both.
- The events change shape on the channel's stream: `ResponseStarted` becomes
  `{"ResponseStarted": {"turn_id": …}}` (a string before); `RetryRequested`, `RetryAccepted` and
  `RetryRejected` carry `target: {"message": …}` or `{"turn": …}` instead of `message`.
  `Message` gains `turn_id` (absent when not set); `RetryRefused` gains `ended`. The web UI is
  changed in the same step, so the API stays compatible with it; other clients that read these
  events have to follow.
- `ChannelError::NotUnderWay` is mapped in `error.rs` to `409 not_under_way`.

### Web UI and TUI

- **Types** (`types/index.ts`): `ResponseStarted` leaves `UnitActorEvent` and becomes an object
  with `turn_id`; `RetryTarget`; the three retry events with `target`; `Message.turn_id`;
  `RetryRefused` with `'ended'`.
- **`ChatView`**: `Answer` keeps its `turnId` from `ResponseStarted`, or from the first message
  of the turn that reaches it after a reload (partial or complete). On the answer under way, where
  `can_retry`, a retry icon calling the new route with the sender and the turn - and, with
  `cancel_actor`, a stop icon for that one answer beside the channel's stop button. `mayRetry`
  on completed messages stays as it is. `RetryAccepted` with a `turn` target removes the
  messages with that `turn_id` from `messages` (shown live during the turn, never part of the
  history, so a reload agrees) and shows the agent as answering; `RetryRejected` words `ended`
  ("its answer had already ended").
- **TUI** (`caelumora-tui`): keeps the turn of the answer under way from `ResponseStarted`;
  `/retry` while one is under way asks for that turn instead of the last answer shown.
- The client crates gain nothing: they are empty, as 234 found.

### Things to watch

- **Nothing of a turn under way is in the history** - the rule everything above rests on: only
  `FullResponseGenerated` writes it (`append_history`), and the agents' `event_filter` admits
  only that. Should a message ever reach the history during a turn, the retry of a turn would have
  to take it back as the retry of a message does.
- **The turn given up for a retry stays recorded**, as every given-up turn (145): its messages on
  an inactive branch, readable through the context of what names them. 237 (erasing) is where
  removing them would go.
- **The retry of a message after a turn given up for a retry:** the `GivenUp` entry makes the
  message before it `NotLast`, as any given-up turn does today. Right: the agent's model was told
  of the calls that ran, and a new answer followed.
- **A turn that produced nothing** leaves no `GivenUp` record (`give_up_turn`); the retry is
  accepted all the same - the run loop knows which turn it gave up. Nothing is told, nothing has
  to be.
- **Approval requests of the given-up turn** are withdrawn (`withdraw_approval_requests`), which
  `give_up` does only for turns that failed or were lowered today.
- **A tool running at the interrupt** is answered as interrupted and told to the model as having
  run (issue 161), classified as a given-up turn's telling is; a plan under way is discarded with
  the turn.
- **The resumed turn** must give its `RunControl` the id, or a retry after a restart never hits.
- **The merged turn** (retry and request in one batch) runs in the request's contexts: the
  `span_id` of the retry is not the one the new answer carries. Acceptable, and the same as a
  request outweighing a retry today.
- **`turn_id` and `turn_entries`** both say which messages belong to a turn; `turn_entries` is
  what the `ContextResolver` counts by and stays. Whether it can be derived from the id later is
  open question 3.
- **Security:** `RetryMessage` covers both targets - "ask for another answer in this channel";
  `CancelChannel` covers `cancel_actor` - the policy that lets someone stop the channel lets them
  stop one actor of it. New tests in the security suite: a retry of a turn under way refused by
  the policy is refused by the user's actor and dropped at the gate; `cancel_actor` is refused
  like `cancel`.
- **Logging:** the turn by id, never content; the dropped request at `warn!` with the id.
- **Markdown and documents:** CHANNELS.md (*Cancelling*: one actor; *Answering again*: an answer
  under way, the merged batch), AGENT-TURN.md (the turn's id; *Beginning and ending a turn*: the
  retry interrupt, `Retried`), REST-SERVER.md (routes, the events' shapes), WEB-UI.md,
  SECURITY.md, ARCHITECTURE.md (*Terms*: turn id).

### Open questions

1. **Does `cancel_actor` keep the channel quiet?** Proposed: no - `quiet` means "nobody is asked
   to respond until woken", which is the channel's, not one actor's; the taken-over answer then
   enters the channel as any answer, and another agent may respond to it. The alternative, quiet
   as `cancel` does, would silence the agents nobody stopped. To be confirmed in step 2.
2. **The snapshot does not say who is answering.** A page loaded during a turn learns of the
   answer with its first message - and of its turn through `Message::turn_id` - but an agent that
   does not stream shows nothing until it ends, and no retry can be offered. The channel snapshot
   could name the turn under way per participant. Proposed: an issue of its own.
3. **Can `ContextRef::turn_entries` be derived from `Message::turn_id`** once every message of a
   turn carries it? It would leave one source for "which messages are a turn's". Proposed: an
   issue of its own, after this one, since messages stored before carry no id.
4. **A stop icon per answer in the web UI**: part of step 2 if small; else `cancel_actor` is
   REST-only at first.

### Steps

Each compiles, passes its tests and can be committed on its own; the documents on the subject are
updated in the step that changes what they describe.

1. **The turn's id - a restructuring, no change of behaviour.** `TurnId`, `Work::turn_id`,
   `Message::turn_id` set where a turn makes a message its own, `RunControl` with the turn under
   way (resumed too), `ResponseStarted { turn_id }`; the web UI's types and `ChatView` reading
   the new shape and keeping `turnId` per answer; the TUI where it matches the event. Tests: every
   message of a turn carries its id, partial ones too, and so does the tool message; a resumed
   turn keeps it; a message's digest is the same with and without it; `ResponseStarted` names
   it. AGENT-TURN.md, REST-SERVER.md.
2. **Cancelling one actor.** `ChannelLoop::cancel_actor`, the route, the security suite, the web
   UI's stop icon per answer (open question 4). Tests in `channel.rs`: one actor's turn ends,
   the others' go on, the channel is not quiet. CHANNELS.md, SECURITY.md, REST-SERVER.md,
   WEB-UI.md.
3. **The retry of an answer under way, in the runtime.** `RetryTarget` in the three events,
   `RetryRefused::Ended`, `GivenUpReason::Retried` and its telling, `Interrupt::Retry` and
   `RunControl::retry`, `Actor::turn_under_way` / `retry_turn`, the run loop's handling with
   `give_up` restructured, the rules of `handle_waiting` (the accepted retry, the merged batch,
   the judged retry beside a request), `UserActor::request_retry_of_turn`,
   `ChannelError::NotUnderWay`, the channel's gate and the interrupt before delivery. Tests in
   `channel.rs` with a model that answers slowly: a retry of the turn under way ends it, nothing
   of it reaches the history or another agent, `RetryAccepted` and a new answer follow; a retry of
   an older message during a turn does not interrupt it and is rejected `NotLast` afterwards; a
   message said during the turn goes into the merged new turn; a retry of a completed message
   beside a request without a message is carried out, not `AskedAgain`; a cancel that won makes
   the retry `Ended`; a turn with a tool running is told of the call; an external agent is never
   interrupted and refused `NotUnderWay`. The security suite. CHANNELS.md, AGENT-TURN.md,
   SECURITY.md.
4. **The retry of an answer under way, for users.** The route and its error, the web UI's retry
   icon on the answer under way, `RetryAccepted` taking the turn's messages off the screen,
   `ended` worded, the TUI's `/retry` for the answer under way. Verified in the browser with a
   model: a long answer asked for again mid-stream, replaced without a trace, and a retry of an
   older message leaving the answer under way untouched. REST-SERVER.md, WEB-UI.md. *From here on
   the issue's "done when" holds.*
5. **Close.** The open questions answered in History or moved to issues of their own (2 and 3
   are candidates), ARCHITECTURE.md (*Terms*), status `done`.

## Done when

A user can ask for another answer while the agent is still answering; only that agent's turn is
ended, and its answer is replaced as a completed one is.

## History

- **2026-10-06** — Opened from open question 5 of [234](234-modify-revoke-and-retry-messages.md),
  which built retry for completed answers only.
- **2026-10-06** — Planned with the maintainer, against the code as of `64ca713`. Decided: the
  retry of an answer under way gives the turn up (issue 145's mechanism) rather than cancelling
  and retrying its result, since nothing of a turn under way has reached the history or the other
  agents; every turn gets a `TurnId`, carried by `Work`, by every message the turn produces and
  by `ResponseStarted` - the latter because not every agent streams; the request names a message
  or a turn (`RetryTarget`); whether a turn can be asked again is decided before it is
  interrupted, so that a retry of an older message never stops the answer under way, and a retry
  that hit its turn is accepted whatever came with it, merged with a request of the same batch
  into one turn; `ChannelLoop::cancel_actor` is built beside it as an operation of its own, after
  `wakeup_actor`, under `CancelChannel`. The plan above.
- **2026-10-06** — Step 1 done, without a change of behaviour. `TurnId` (`caelumora_model::work`),
  `Work::turn_id` - given by `Work::new`, stored with the work, a new one for work stored before -
  and `AgentChannelState::begin_turn` returns it. `Message::turn_id`, outside every digest and not
  taken over by `Message::modify`: `TurnState::push` sets it on everything the turn keeps, `stamp`
  on what the model says, partial and complete; the tool message of `ToolCalls::collect` and the
  message a given-up turn tells its model with get it where they are built, since what is sent of
  them is built before it is kept. The external agent builds its `Work` before it announces the
  answer now, so that `ResponseStarted` can name it, and gives its partial and recorded messages
  the work's id - including what a long task says later, the work that began it being known; an
  external agent without a channel thus fails before `ResponseStarted` rather than after it.
  `ResponseStarted { turn_id }`. `RunControl` keeps the id with the turn's token
  (`begin_turn(turn)`, `RunControl::new(state, turn)` for a resumed one) and answers
  `turn_under_way()`; until step 3 asks it, `AgentActor::cancel` logs which turn it cancels. The
  web UI reads `ResponseStarted` as an object and keeps `turnId` per answer under way, from the
  event or from the first message of the answer; the TUI matched no `ResponseStarted`. Tests: every
  message of a planning turn, shown, published, in the channel's history and in the agent's
  transcript, carries the id `ResponseStarted` named, and an external agent's alike; a partial
  message stamped is the turn's; a given-up turn's telling is; work and a stored turn keep their
  id; `RunControl` knows the turn under way, a resumed one too; the digest ignores the id.
  AGENT-TURN.md (*The `Turn`*), REST-SERVER.md (*Streams*), WEB-UI.md.
- **2026-10-06** — Step 2 done. `ChannelLoop::cancel_actor(context, actor)` beside `wakeup_actor`:
  `CancelChannel` first, then `Actor::cancel` of that actor alone. Open question 1, decided with the
  maintainer: the channel does not become quiet - the others are still asked to respond, and the
  answer the turn is taken over with reaches everyone as any answer does. An actor that is not in
  the channel is `ChannelError::NotAParticipant`; `wakeup_actor` answered the same case with an
  untyped error and now gives the same one, a difference without a reason. `POST
  …/participants/{aid}/cancel` (`cancel_participant`) maps it as `remove_participant` does the
  participant, `404 participant_not_found` and `403 cancel_forbidden`. Open question 4: the web UI
  has a stop icon on every answer under way, beside the channel's stop button - small enough for
  this step. Tests in `channel.rs`: one actor's turn ends, the other's does not, and the next
  message is answered; one actor is refused under a policy that refuses cancelling, before it is
  looked up. The security suite (S4): bob may cancel one actor as a participant, asked as
  `CancelChannel`; carol is refused without learning whether the actor is there. CHANNELS.md
  (*Cancelling*), SECURITY.md, REST-SERVER.md, WEB-UI.md. Not verified in the browser: the stop
  icon calls the route that the tests cover, and step 4 verifies the answers under way with a
  model.
- **2026-10-06** — Step 3 done. `RetryTarget { Message(id), Turn(id) }` in the three retry events,
  `RetryRefused::Ended`, `GivenUpReason::Retried` ("your answer was asked for again", told with the
  calls that ran). `RunControl::retry(turn)` compares the turn with the one under way under the
  turn's lock and leaves `Interrupt::Retry`; `turn_under_way()` and `retry` count only a turn that
  can still be ended (`Planning`, `ExecutingTools`, `WaitingForApproval`), as `cancel` already did,
  and a new turn drops a leftover retry as it drops a leftover cancel. `Actor::turn_under_way` and
  `retry_turn` (defaults `None` and `false`, the external agent keeps them). The run loop gives a
  turn interrupted so up and remembers it until a turn begins; `give_up` withdraws the approval
  requests of every turn that stays in the channel, not only of one that failed or was lowered.
  `handle_waiting` follows the rules above, which changes 234's: a retry of a message beside a
  request to respond is judged instead of rejected unseen. `UserActor::request_retry_of_turn`
  (`RetryMessage` first, then `NotAParticipant`, then `NotUnderWay`) shares asking and sending with
  `request_retry`; `ChannelLoop::turn_under_way`; the channel admits a turn's request for an agent
  of the channel and ends the turn as it delivers it (`route_event_impl`), dropping it if the turn
  is over. Against the plan: `AgentChannelState::retried` keeps taking a message - the turn is the
  run loop's to decide - and `ChannelError::NotUnderWay` is mapped to `409 not_under_way` already,
  since the mapping is exhaustive; the web UI's types follow the new shapes (`target`, `ended`,
  `retried`), its behaviour stays for step 4. A cancel that came first and makes a request `Ended`
  is covered by `RunControl`'s tests only: in a running channel the two cannot be ordered so. Nor
  is a retry of a message beside a request without a message: no request without one reaches an
  agent in the tests' channels. Tests: an answer under way asked for again is given up, nothing of
  it reaches the channel, and a turn of its own answers, asked what the given-up one was; what was
  said during it is answered in that one turn; an older answer asked for again leaves the answer
  under way alone and is rejected `NotLast` beside the request that followed, which is answered; a
  turn asked for again while a tool runs tells the next one of the call, recorded as the turn's;
  one waiting for an approval withdraws the request; an external agent has no turn to ask for
  again; the security suite (M2). CHANNELS.md (*Answering again*), AGENT-TURN.md, SECURITY.md,
  REST-SERVER.md.
- **2026-10-06** — The decision of a request to answer again moved into `AgentActor::retry` for all
  three cases (the turn given up for it, another turn, a message), on the maintainer's request to
  simplify `handle_waiting`, which now only picks the request it judges. Of two requests of a turn
  not given up only the last is rejected as `Ended`, the other as asked again.
- **2026-10-06** — Step 4 done. `POST …/participants/{aid}/retry` with `{turn_id}`
  (`RetryTurnRequest`), through `UserActor::request_retry_of_turn`; `409 not_under_way` was mapped
  in step 3. The web UI offers a retry icon on an answer under way once its turn is known and
  `can_retry` allows it, beside the stop icon of step 2; `RetryAccepted` of a turn removes the
  messages of that turn shown so far, by `turn_id`. The TUI keeps the agent and turn of the last
  answer under way from `ResponseStarted`, forgets them as it ends, and `/retry` asks for that
  answer while there is one, else for the last answer shown. Verified in the browser against the
  REST server with `examples/config.yaml` and `gemma4:26b`, session `simple-session`: an essay
  asked for again mid-stream from its icon was replaced at once by a new answer under way, the
  server logging the turn given up; the channel's history then held the question and the new
  answer alone, with a new `turn_id`; asking for the given-up turn again over REST was refused
  `409 not_under_way`. A second essay kept streaming while the older answer was asked for again
  over REST, and once it ended the notice "did not answer again: more was said or done since that
  answer" followed. *From here on the issue's "done when" holds.* REST-SERVER.md, WEB-UI.md,
  CHANNELS.md.
- **2026-10-06** — Step 5, and the issue done. The open questions of the plan:
  1. `cancel_actor` does not make the channel quiet, as proposed and decided with the maintainer
     (step 2).
  2. The snapshot does not say who is answering:
     [240](240-answers-under-way-not-in-snapshot.md).
  3. Whether `ContextRef::turn_entries` can be read from `Message::turn_id`:
     [241](241-turn-membership-recorded-twice.md).
  4. The web UI's stop icon per answer came with step 2.

  ARCHITECTURE.md, which has no section *Terms* as the plan said, has the turn's id and the stop
  and retry of an answer under way in *How a message travels*.
- **2026-10-08** — Changed by [258](258-what-was-said-is-lost.md): an answer under way asked for
  again keeps what it said - in the agent's chains and in the channel's history, followed by its
  end with the reason `Retried` - and the new answer follows it rather than replacing it. The users
  are told with `TurnGivenUp`; the web UI no longer takes the turn's messages off the screen. The
  rule this issue rested on, that nothing of a turn under way is in the history, holds no more;
  nothing had to be taken back all the same, since what was said stays. Asking again after a turn
  given up is [259](259-answer-again-after-a-turn-given-up.md).
