# 259 — An agent cannot be asked to answer again after a turn it gave up

**Status:** in progress
**Area:** Sessions and channels

## The problem

Since [258](258-what-was-said-is-lost.md), a turn given up keeps what it said, followed by its end.
Asking for another answer then has nothing to aim at: the retry of a message refuses a message
followed by a turn's end as `NotLast` (`AgentChannelState::retried`), and the retry of a turn
(`RetryTarget::Turn`) only reaches a turn still under way - one that ended is refused as
`NotUnderWay`. After a turn failed - the model unreachable for a moment, say - the user can only
write something again, or wake the agent.

## Why it matters

A failed turn is the case where answering again is wanted most, and the web UI offers no icon for
it: the notice of the turn given up has none, and the message before it is not the agent's last
answer any more.

## Considerations

- **A turn given up is asked for again as a turn:** `RetryTarget::Turn` of a turn that ended given
  up is accepted as a request to answer anew after it, taking nothing back - its messages stay, as
  258 decided. Its answer is then a turn of its own, in the contexts of the request.
- **Or the notice offers a wakeup** of the agent, in the request's name: no new concept, but the
  agent does not know it is to answer the same question.
- What the model is given is decided already: the turn given up and why stand in its context.

## Plan

Written against the code as of `3c4aeb2`. Names are proposals; what is fixed is where each rule
lives.

### A. What the code shows

- **The retry of a message** is judged by the agent on its transcript
  (`AgentChannelState::retried`): anything after the message but changes of it and the rest of its
  turn makes it `NotLast` - and since [258](258-what-was-said-is-lost.md) the end of a turn given
  up follows its messages. A turn that failed before it said anything has no message to aim at.
- **The retry of a turn** is refused unless the turn is under way: by the user's actor
  (`request_retry_of_turn`, `ChannelError::NotUnderWay`, REST `409 not_under_way`), by the channel
  as it delivers - `Actor::retry_turn` interrupts the turn and answers `false` for any other, which
  drops the request - and by the agent, which accepts only the turn it gave up for the request
  (`given_up_for`), else `RetryRefused::Ended`.
- **The channel's history knows whose turn was given up and why** (`HistoryEntry::TurnGivenUp
  { turn_id, actor, reason }`, 258 step 6), the agent's transcript the same of its own
  (`TranscriptEntry::TurnGivenUp`).
- **A new answer is begun from the retry's envelope** when no request to respond came with it
  (`handle_waiting`, issue 235): in the contexts of whoever asked.
- **The web UI** offers a retry on the answer under way (`retryAnswer`, with its turn) and on the
  last stored message (`mayRetry`); the notice of a turn given up has none. The TUI's `/retry` asks
  for the turn under way, else for the last answer it showed.

### B. Decisions

1. **A turn given up is asked for again as a turn.** `RetryTarget::Turn` names a turn under way, as
   now, or the agent's last turn, given up: then it is a request to answer anew after it. Nothing is
   taken back - what it said stays (258) - and nothing is interrupted. One target, one route, one
   permission (`RetryMessage`), as 235 decided for the turn under way.
2. **For which reasons** (decided with the maintainer): `Failed` and `Lowered`. A turn given up as
   `Retried` is answered anew already; one `Stopped` belongs to an agent that left. The rule is
   the reason's: `GivenUpReason::may_be_answered_again`, in the model, read by everyone who decides.
3. **Only while nothing followed it** (decided with the maintainer): the end of the turn is the last
   thing in the channel's history, and in the agent's transcript - as a message is asked for again
   only while it is the agent's last answer (`NotLast`). Once something was said after it, the user
   writes rather than asks again: the conversation moved on, and the request would be ambiguous.
4. **Who decides what, as for a message.** The user's actor and the channel decide from the
   channel's history - what the user sees; the agent decides from its transcript - what it knows -
   as `retried` does. The two read different chains, so the rule of the reason is shared and the
   reading is each chain's own: `MessageHistory::turn_given_up_last(author)` and
   `AgentChannelState::answers_again_after(turn)`.
5. **`ChannelError::NotUnderWay` becomes `TurnNotRetryable`**, REST `409 turn_not_retryable`: a
   turn neither under way nor the agent's last turn given up. Nothing reads the old code (the web
   UI does not); the name would say less than it means otherwise.
6. **An external agent** keeps refusing (`RetryRefused::Elsewhere`): its remote side keeps what it
   was given. `can_retry` keeps the icon off its notices, as off its messages.

### C. The model (`caelumora-model`)

- `turn.rs`: `GivenUpReason::may_be_answered_again(&self) -> bool` - `Failed` and `Lowered`.
- `message_history.rs`: `turn_given_up_last(&self, author: ActorId) -> Option<(TurnId,
  GivenUpReason)>` - the last item of the active path, if it is a turn of `author` given up.
- `agent.rs`: `answers_again_after(&self, turn: TurnId) -> Result<(), RetryRefused>`: the
  transcript's active path ends in `TurnGivenUp` of `turn` with a reason that may be answered
  again; `NotLast` if something follows it, `Ended` for another reason, `NotItsAnswer` if the
  transcript holds no end of that turn.
- Tests: each, with every reason and with something said after.

### D. The runtime (`caelumora-runtime`)

- `channel.rs`: `ChannelLoop::retryable_turn(author, turn) -> bool` - under way
  (`turn_under_way`), or the last turn given up that may be answered again; `ChannelError::
  TurnNotRetryable`. Where the request is delivered (`route_event_impl`), a turn under way is
  interrupted as now (`retry_turn`); one given up is delivered without; any other is dropped and
  logged.
- `actor/user.rs`: `request_retry_of_turn` asks `retryable_turn`, refuses as `TurnNotRetryable`.
- `actor/agent/mod.rs`: `retry` accepts `RetryTarget::Turn(turn)` that is not `given_up_for` if
  `answers_again_after(turn)` - `RetryAccepted`, nothing taken back - and `handle_waiting` begins
  the new answer from the retry's envelope, as for a turn given up for the request.
- Tests (`channel.rs`): a turn that failed is answered anew on request, its messages and end stay,
  `RetryAccepted` and a new answer follow; refused once something was said after it, for a turn
  given up as `Retried`, for another agent's turn, for a turn that is not the last given up; the
  external agent refuses as `Elsewhere`.
- Security suite: the retry of a turn given up is refused without `RetryMessage`, by the user's
  actor and at the gate - as for the turn under way.

### E. REST, web UI, TUI

- REST: `409 turn_not_retryable` in place of `not_under_way` (`error.rs`); the route's doc.
- Web UI: the notice of a turn given up as `failed` or `lowered` has a retry icon while it is the
  last thing in the chat, nothing is under way and the member's `can_retry` holds
  (`api.retryAnswer` with the notice's `turn_id`); `RetryRejected` is worded as before.
- TUI: `/retry` after a turn given up asks for that turn, from the `TurnGivenUp` it showed.

### F. Steps

1. **The model and the runtime** (C, D): the rule, the gate, the delivery, the agent, the tests,
   the security suite. CHANNELS.md (*Answering again*), AGENT-TURN.md where it names the retry of
   a turn.
2. **For users** (E): REST, web UI, TUI. REST-SERVER.md, WEB-UI.md. Verified in the browser with a
   model that fails once.
3. **Close**: history, status `done`.

### G. What to watch

- **The two chains agree** on "the last thing" only as far as the agent heard what the channel
  holds: a message the agent did not hear - asleep, say - is in the channel's history and not in
  its transcript. The channel then refuses, and the agent never asks. Both refuse alike the other
  way round: what the agent perceived the channel holds.
- **The new turn's context** holds the turn given up and why (258, the `ContextResolver`): the model
  does not repeat a call it is told ran, nor the one that failed without knowing it did.
- **Lowered**: the new answer is a turn in the channel as it is now, bound by its classification.
- **A request to answer again while a turn is under way** that names an older turn given up is
  refused: the end of that turn is no longer the last thing.

## Done when

- A user who may ask an agent to answer again can do so after a turn the agent gave up, and the web
  UI offers it on the turn's notice.

## History

- **2026-10-08** — Opened when closing [258](258-what-was-said-is-lost.md), from part J of its plan.
- **2026-10-08** — Planned, against the code as of `3c4aeb2` (parts A to G): `RetryTarget::Turn`
  names the agent's last turn given up as well; `NotUnderWay` becomes `TurnNotRetryable`. The
  maintainer confirmed the two proposals: `Failed` and `Lowered` only, and only while nothing
  followed the turn's end.
- **2026-10-08** — Step 1 done: `GivenUpReason::may_be_answered_again`,
  `MessageHistory::turn_given_up_last` and `AgentChannelState::answers_again_after` in the model;
  `ChannelLoop::retryable_turn` for the user's actor, the delivery delivering a turn given up
  without interrupting it, and the agent accepting it in `retry`. `ChannelError::NotUnderWay` is
  `TurnNotRetryable`, and with it REST's `409 turn_not_retryable` - in this step rather than the
  next, since the code names the error. Tests: the rule in the model for every reason and with
  something after; a turn that failed answered anew and kept; the refusal once something followed;
  the security suite asks the same question before the history is looked at. CHANNELS.md and
  REST-SERVER.md.
- **2026-10-08** — Step 2 done: the web UI's notice of a turn given up, failed or lowered, has a
  retry icon while it is the last thing in the chat, nothing is being generated and the member's
  `can_retry` holds (`mayRetryGivenUp`, the route of the turn under way with that turn); the TUI's
  `/retry` asks for the turn given up it was told of last, if nothing was said since. REST was done
  in step 1. Not verified in the browser.
