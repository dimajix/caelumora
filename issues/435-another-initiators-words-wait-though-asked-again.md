# 435 — A turn that waits at another's words stays deaf though its own initiator asks again

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** An agent whose personality hears during a turn with `others: wait` hears nothing more in that turn once someone other than its initiator has spoken, even when its initiator asks it again afterwards - by mentioning it or otherwise - though between turns that request would let it hear the other's words.

## The problem

Issue [431](431-hearing-during-a-turn.md) lets an agent take in, between the steps of a turn, what
its inbox holds up to the first event it may not take (`AgentBody::take_in_during_turn`,
`EventReceiver::try_recv_if`, `HearsDuringTurn::takes`). With `others: wait`, the default, a
message said in another's authority than the turn's is such a barrier: Bob may not steer what the
agent does in Alice's authority. Everything behind the barrier waits for the turn's end, in order.

So in a channel of several people, once Bob has spoken, the agent hears nothing more in Alice's
turn - also not what Alice says to it afterwards, though she addresses it explicitly, e.g. by an
`@`-mention: "@helper, ignore that, go on with the other file" reaches the turn only when it has
ended.

The maintainer asked whether such a barrier could be lifted once a later message of a user
explicitly mentions the agent.

## Why it matters

- The turn's own initiator loses the means 431 gave them - steering the agent at work - as soon as
  anyone else speaks, which in a channel of several people is soon.
- The rule is stricter during a turn than between turns: there, Bob's words reach a turn in Alice's
  authority when Alice asks after them
  ([222](222-queued-requests-answered-one-by-one.md)) - she could have read them, and asks
  regardless.

## Considerations

### Which barriers a later request can lift

The barriers have different reasons; a later request answers only one of them:

| Barrier | Why it is one | Lifted by a later request? |
|---|---|---|
| Words in another initiator's authority (`others: wait`) | the other would steer a turn acting in an authority they do not have | **yes**, by a request in the turn's own authority |
| A message above the turn's classification | the turn would carry it below its level | no - a rule of security |
| A command | it is carried out where it came | no - a rule of order |
| A change to a message, a request to answer again | not taken in during a turn yet (431) | no - a question of its own |

### In whose authority

Only a request in the turn's own authority lifts the barrier. That is the rule between turns
(222): a turn hears what was said before the request it answers, and acts in the authority of who
asked. If Bob mentions the agent, his request is in his authority: it must not lift the barrier at
his own words - that would be exactly what `wait` prevents - and it waits, as today, for a turn in
his authority after this one.

### The signal: the request, not the mention

A mention does one thing: the `PolicyModerator` asks every agent a message mentions to respond
(`mentioned_responders`), with a `ResponseRequested` in the authority of whoever mentioned it.
431 already rejected the mention as a signal for hearing - whom to ask is the moderator's, and the
request carries the authority.

- **(a) The request in the turn's authority (recommended).** Covers the mention, and also a
  channel where the moderator asks the agent without one - a channel of one agent, where every
  message of Alice asks it. Independent of how a moderator decides whom to ask.
- **(b) The mention itself** (`Message::mentions`). Ties the rule to one moderator's way of
  asking, and does not carry the authority by itself: it would have to be read off the sender,
  which the request already does.

### Taking past what waits

Alice's request lies behind Bob's message and Bob's request; the prefix that `try_recv_if` takes
ends at Bob's message. What is needed:

1. **Looking ahead without waiting.** `try_recv_if` looks at the next event only; `recv_matching`
   searches past it, but waits. Rather than a third method, `try_recv_if` is generalized: the next
   event `take` takes, passing over the events it may leave where they are, and never past an
   event that has to stop it. Today's `try_recv_if` is the case that passes over nothing - one
   method, not two near-identical ones. On the history path (issue
   [223](223-full-inbox-drops-events.md)) that may mean reading on a page or more.
2. **What it takes, in order; what it passes over, left in place.** Once a request in the turn's
   authority lies ahead before the first hard barrier, the turn takes everything up to it - Bob's
   words too, perceived in the order they were said - and passes over Bob's request, which stays
   in the inbox. So the transcript is not reordered: only a request is passed over, and requests
   are not in the transcript. Taking ahead is what `Heard` already supports (`took_ahead`, as for
   `recv_matching`), and the request staying in the inbox keeps it across a restart - unlike the
   request kept by the actor that 431 rejected.
3. **Alice's request** is dropped as answered by the turn, as every request in its authority
   taken during a turn is; the prompt part placed after what was heard asks the model to answer
   it.
4. Behind Alice's request, the rule applies anew: Bob's words said after it wait, unless Alice
   asks again after them.

### The setting

- **(i) A third value `others: when_asked` (recommended).** `wait` stays as it is and the default,
  so nothing changes for a personality that names nothing; the rule is tried personality by
  personality, as 431 was.
- **(ii) `wait` changes its meaning** to "until the turn's initiator asks again". Fewer values, but
  changes what existing personalities do, and leaves no way back to the stricter rule.

The rule is the model's, next to `HearsDuringTurn::takes`: which events stop, which may be passed
over, and whether a request lifts what waits.

## Decisions for the maintainer

1. The signal: the request in the turn's authority (a, recommended), or the mention (b)?
2. The setting: a third value `others: when_asked` (i, recommended), or `wait` changing (ii)?
3. The receiver: `try_recv_if` generalized to pass over what may wait (recommended), rather than a
   method of its own?

## Estimate

With the recommendations, about the size of one of the larger steps of 431, in four steps:

1. **The setting** (`caelumora-model/src/agent.rs`): `OthersWords::WhenAsked`, the REST types of
   personalities, the web UI where it edits them. Small, no change of behaviour.
2. **The receiver** (`caelumora-runtime/src/transport`, `channel/inbox.rs`): `try_recv_if`
   generalized, in `InboxReceiver` - queue, what was read of the history, the history - and in
   `InMemoryEventReceiver`, with their tests. Medium; the care is in the history path and in
   `Heard`.
3. **Taking in** (`actor/intake.rs` and `HearsDuringTurn`): the rule which events stop, which are
   passed over, and when a request lifts the barrier. Small to medium.
4. **Tests and documents.** Channel tests: Alice's mention after Bob's message lets the turn hear
   both, Bob's request answered after the turn in his authority; Bob's mention lifts nothing;
   with `wait` nothing changes; a command between them stays a barrier. In the agents' security
   suite: a turn hears another's words only once its own initiator asked after them.
   AGENT-TURN.md (*Hearing during a turn*). Medium.

Nothing stored changes; a personality without the new value reads as before.

## Done when

- An agent whose personality says `others: when_asked` hears, in a turn, the words of another said
  before a request in the turn's own authority, in order, and answers that request with the turn;
  the other's request waits for a turn in their authority.
- A request in another's authority, a mention by them included, lifts nothing.
- Tests show both, the security suite the rule of authority.

## History

- **2026-10-20** — Opened at the maintainer's request, who asked whether a barrier could be lifted
  once a user explicitly mentions the agent. Analysed: only the barrier at another initiator's
  words can be, and only by a request in the turn's own authority - the rule between turns (222);
  the request is the signal rather than the mention, since the mention only makes the moderator
  ask. Recorded with the recommendations: the request as the signal, a third value
  `others: when_asked`, and `try_recv_if` generalized to pass over what may wait.
- **2026-10-20** — Asked by the maintainer to implement it with the recommendations as the
  decisions: the request in the turn's authority as the signal, `others: when_asked` beside `wait`,
  `try_recv_if` generalized. Implemented on the branch `issue-435-hearing-when-asked` in the four
  steps of the estimate: `OthersWords::WhenAsked`; `try_recv_if` taking a `Pick` per event - take,
  pass over, stop -, `InboxReceiver` looking through queue, what was read of the history and the
  history in one pass for every way of receiving; `HearsDuringTurn::pick` and `pick_request`, what
  both check of a message shared with `takes` as `hears_said`, applied by
  `AgentBody::take_in_during_turn`, which after a stop looks on for a request in the turn's
  authority, takes it ahead, and then takes what came before it. A request not recorded is
  stopped at: without a position it cannot say what came before it. Tests in the model, the
  transport, the inbox, the intake and the agents' security suite, each checked to fail with the
  fault it is meant to catch; AGENT-TURN.md (*Hearing during a turn*). No personality of the seed
  uses the new value yet. Not yet tried with a real model.
