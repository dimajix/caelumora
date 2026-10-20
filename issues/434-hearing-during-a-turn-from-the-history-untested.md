# 434 — Hearing during a turn is not tested with an inbox that reads from the history

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** That an agent hears between the steps of a turn (issue 431) is tested only with an inbox whose queue holds everything, so nothing shows that it works the same when the inbox is behind and reads from the channel's history, or after a restart in the middle of a turn that heard something.

## The problem

Issue [431](431-hearing-during-a-turn.md) lets an agent take in, before every model call of a
turn, what its inbox holds up to the first event it may not take (`AgentBody::take_in_during_turn`,
`EventReceiver::try_recv_if`). Issue [223](223-full-inbox-drops-events.md) made the inbox a queue
with the channel's history behind it: once the queue fell short - an overflow, a join, a restart -
the inbox reads on from the history (`InboxReceiver`), and an event left where it is may lie in the
queue, in what was read of the history (`InboxReceiver::read`) or in the history itself.

Looked at in the code, the two fit: `HearsDuringTurn::takes` asks only what an envelope read from
the history carries as it carried it live - the initiator, which the history records, and the
sender and classification of the message -, not the sender's security context, which differs
between the two. Where the agent stands (`heard`) and what it heard are stored together, and a turn
resumed after a restart answers a request it dropped as heard.

But the tests show it only in parts:

- `InboxReceiver`'s unit test `the_next_is_taken_only_if_it_may_be` takes up to a barrier from an
  inbox opened behind - the history path of `try_recv_if`, without an agent.
- The channel's tests of hearing during a turn (`told_while_calculating` and the tests using it)
  run with a queue that holds everything.
- `an_agent_whose_turn_outlasts_its_inbox_hears_all_of_it_afterwards` overflows the inbox during a
  turn, but with `HearsDuringTurn::default()`: the agent hears nothing during the turn.
- `what_an_agent_was_delivered_and_did_not_store_before_a_restart_is_delivered_again_once`
  restarts in the middle of a turn, again without hearing during it.

## Why it matters

The combination is the case the two issues were built for: a long turn, in which much is said, is
both the turn worth steering and the one whose inbox overflows. A change to either - to the
predicate, to where the agent stands, to when it stores - can break the combination while every
test passes.

## Considerations

Channel tests with a scripted model, in `channel_tests/agent.rs`, for an agent that hears during a
turn (`from: users`):

- **Behind, then heard.** More is said during a tool step than the queue holds; the next step is
  given all of it, in order, once, and no turn follows for the requests among it.
- **A barrier with a backlog behind it.** A command (or the words of another initiator with
  `others: wait`) early in what is said, more than the queue holds behind it: the next step is
  given what came before the barrier only; after the turn, the command is carried out where it
  came and the rest is taken in, in order, once.
- **A restart after hearing.** The channel stops after a step took in a message and its request,
  and runs again: the turn resumes, the message is given once, it is not delivered again, and no
  second turn answers the request.

## Done when

The channel's tests show an agent that hears during a turn taking in, in order and once, what its
inbox reads from the history, stopping at a barrier there as in the queue, and keeping what it
heard across a restart.

## History

- **2026-10-20** — Opened after a review of how issues 431 and 223 work together: they fit, but
  the tests do not show the combination - neither hearing from an inbox that is behind, nor a
  restart after a turn heard something.
