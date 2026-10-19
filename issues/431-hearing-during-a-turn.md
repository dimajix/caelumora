# 431 — An agent hears nothing of what others say while its turn runs

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** An agent takes in what others say only between its turns, so a turn of many steps - tool calls, a plan - works on to its end without what was said meanwhile: a correction, an addition, an instruction to stop doing something reaches it only after it has done it.

## The problem

An agent reads its inbox in two places (`AgentActor`, `actor/agent.rs`; see
[AGENT-TURN.md](../docs/AGENT-TURN.md), *the cognitive cycle*):

- **Between turns**, `Idle` and `Sleeping` take the next event and everything queued behind it
  (`AgentBody::take_in`, issue [222](222-queued-requests-answered-one-by-one.md)).
- **In `WaitingForUser`**, `recv_matching` takes only approvals and answers to questions; everything
  else is put aside in the receiver's buffer, in order, until the turn is over.

In `Planning`, `Busy` and `ExecutingTools` nothing is read. A turn of many steps - a model that
calls tools round after round, a plan of several steps (issue 412 allows `MAX_MODEL_CALLS` model
calls per step) - may run for minutes, with a slow local model longer. Whatever the others say in
that time - "not that file, the other one", "also check X", "stop, that is wrong" - is perceived
only when the turn has ended: the agent finishes its work on what it knew when the turn began, and
then a second turn answers what was said in between.

The maintainer wants agents to take in what other participants say between the steps of a turn,
so that they can take it into account in their next steps.

## Why it matters

- A participant cannot steer an agent at work. The only means are `cancel` - which throws the work
  of the turn away - or waiting, and then asking again.
- It is the same lag as [222](222-queued-requests-answered-one-by-one.md), within a turn: the
  agent answers a conversation that has moved on, only now it is the turn's own steps that are
  stale, and their tool calls have effects.
- The longer a turn, the fuller the inbox in the meantime
  ([223](223-full-inbox-drops-events.md)): reading during a turn also empties it more often.

## What already fits

The model side is largely prepared for it:

- `AgentChannelState::perceive` already takes in during a turn: what is perceived is appended to
  the transcript, the turn has seen it (`Work::see_message`), and the turn's context moves on past
  it (`work.context.transcript`, issue 260). `note_announced` (who joined or left, issue 408) does
  the same. The next model call resolves its context from the transcript as the context names it
  (`Turn::full_context`, `ContextResolver`), so a message heard between two steps is given to the
  model in the next one, in its place, after what the turn said so far.
- What the model is given grows only at its end: the prompt cache keeps its prefix.
- The turn's own messages are found by their `turn_id`, not by their place in the transcript: a
  turn given up as `Lowered` reduces exactly its own messages (`ContextResolver`), and
  `AgentChannelState::retried` takes back what follows the answer asked for again. Heard messages
  between the steps of a turn do not confuse either.
- What may not stand between the calls of a step and their results already has a rule: what a
  tool gives while the calls run is put aside and written after the step's tool message
  (`TurnState::given_after_step`, issue 389).
- The receiver already keeps what is not to be taken now, in order, in its own buffer
  (`InMemoryEventReceiver::recv_matching`), and hands it out first later.

What is missing is in the actor: a point in the turn where it reads its inbox, and a rule for
what it takes there.

## Considerations

### Where in the turn

- **A - before every model call (recommended).** At the start of `Planning`, once the turn may
  call its model (`may_call_model`) and before compaction is judged (`compaction_due`), so that
  what was heard counts for the size of the context. At that point every call of the previous step
  has its result and the tool message is said: nothing can come between a call and its result.
  It covers every boundary the maintainer means - between tool rounds, between the steps of a
  plan, before its final answer - with one place in the code, and the model sees what was said in
  exactly the call that follows.
- **B - also while tools run or the turn waits for a person.** Reading would empty the inbox more
  often, but what is read could not go into the transcript before the step's tool message: it
  would have to be put aside like `given_after_step` and is seen by the model no earlier than with
  A. The benefit is the queue's, not the agent's - and [223](223-full-inbox-drops-events.md) solves
  the queue. Not now.
- **C - interrupting a model call** when something is said, and calling again with it. Steers the
  fastest, but throws away the call under way - every message in a busy channel would cost a call
  - and makes the turn's progress depend on how much is said. Rejected; `cancel` remains for that.

### Which events, and what is done with them

What reaches an agent at all is its `event_filter`. In a turn, at the point of A:

| Event | Between turns (today) | Between the steps of a turn (proposed) |
|---|---|---|
| `CompleteMessageGenerated` | perceived | perceived - subject to the two barriers below |
| `ParticipantJoined`, `ParticipantLeft` | noted (`note_announced`) | noted, as today |
| `ResponseRequested` | the last begins a turn | taken in and dropped, as answered by this turn (below) |
| `MessageModified`, `MessageRevoked`, `MessageReclassified` | recorded in the transcript | a barrier at first (below) |
| `RetryRequested` | judged; may begin a turn | a barrier: it waits for the turn's end, as today |
| `CommandRequested` | carried out where it came (issue 269) | a barrier: carried out after the turn, as today |
| approvals, answers to questions | dropped as late | none is awaited at that point: a barrier, as today |

A **barrier** is an event that is not taken during the turn: it and everything behind it stay in
the inbox, in order, and are taken in after the turn by `take_in` as today. Taking only a prefix of
the queue keeps the order every rule in `take_in` relies on - a command is carried out after what
was said before it and before what was said after it (issue 269), a change follows the message it
changes. Taking past a barrier would need each of those rules to be re-examined for a reordering;
the barriers are rare (a command, an edit, an answer asked for again), and after one the agent is
simply as deaf as today until its turn ends.

### A request taken in during a turn

The moderator sends the request to respond right after the message that asks for it
(`PolicyModerator`). If the message is heard between two steps, the call that follows already
answers it; a turn begun afterwards on the request would answer it a second time, on a
conversation it has heard all of - [222](222-queued-requests-answered-one-by-one.md) in small. So a
request taken in during a turn is dropped: the turn answers it. Since A takes in right before a
call, a call does follow - unless the turn is cancelled or given up during it, and then the request
was no more answered than the one the turn began with.

### Whose words a turn hears

A turn acts with the authority of whoever asked (its `SecurityContext`, decided in 222 as the
authority of the last request: "never of someone who did not ask"). Heard between its steps, the
words of someone else steer tool calls made with an authority that person does not have: Bob says
"and delete X" while the agent works for Alice, and the next step deletes X in Alice's name. Today
Bob's words reach a turn only if Alice asks after he said them; she could have read them.

- **(i) Hear everything.** Simple, and "a turn sees what others said" holds for every turn already.
  But it opens a confused deputy that does not exist today, without anyone of authority asking in
  between.
- **(ii) A barrier at the words of another initiator (recommended).** A message whose envelope's
  `initiator_id` is not the turn's is a barrier: it, its request and everything behind them wait
  for the turn's end, and are answered in a turn in that person's authority as today. The
  initiator stays the same along a chain of agents (issue 45), so in a channel of one user and
  several agents everything is heard; with several users, a turn hears its own user and the agents
  working for them. The security suite states the rule.
- **(iii) The turn narrows its authority** to what both may. Exact, but `SecurityContext` has no
  intersection of authorities, and the tools offered at the turn's start (`offer_tools`) would have
  to be judged anew mid-turn. Not without a need.

Joins and leaves carry no authority and are noted whoever caused them.

### What a turn may read

A turn's label is the join of what it has seen, *capped* at the classification the channel had
when the turn began (`Work::see`, `join_within`). Between turns that cap is harmless: a turn
begins with the channel's classification as it is. During a turn, a channel raised in the
meantime may carry messages above the turn's classification; heard, they would be seen under a
label that understates them, and what the turn writes - notes, memory - would carry them below
their level. So a message classified above the turn's classification is a barrier too: the next
turn, begun at the channel's classification, hears it. The same holds for a change of a message,
once changes are taken in.

### Changes to messages

`Transcript::modify`, `revoke` and `reclassify` append an entry; between turns that is all they
need. During a turn, the turn's context would also have to move on past the entry, and the turn
see what changed - as `perceive` does. A reclassification upward of a message the turn has already
used raises the question what becomes of what the turn derived from it before - a question of
its own beyond [239](239-reclassified-message-not-in-agents-state.md), which settled
reclassification between turns, not to be answered here in passing. Proposed: the
three are barriers at first; taking in a modification and a revocation can follow, with the
context moved on, if the barrier turns out to hurt.

### Events that cannot be taken in now: sent to itself, or left in the inbox

The maintainer suggested that an agent might send itself again what it cannot handle during a
turn, to have it back in its inbox. Looked at in the code:

- **It would not arrive.** The channel delivers nothing back to its sender
  (`ChannelLoop::route_event_impl`), and the agent's output goes through the moderator to the
  others. It would need a path of its own past the moderator.
- **It would carry the wrong authority.** What an actor sends is stamped as its own
  (`OriginEventSender`); a request sent again would be the agent's, not the asker's - or the agent
  would have to forge envelopes in someone else's name, which the security model forbids.
- **It would break the order.** What is sent again lands behind what came meanwhile: a command
  behind messages said after it, a change behind a message said after the one it changes.
- **It would be stored twice** once the inbox is part of the channel's state
  ([223](223-full-inbox-drops-events.md), [385](385-inbox-lost-on-restart.md)): a delivery taken
  in, committed, and delivered anew.

What it wants - that an event not handled now is handled later, where it came - the receiver can
give without a round trip: the event is not taken out of the inbox at all. `EventReceiver` gets
`try_recv_if(predicate)`: the next event if it fulfils the predicate, otherwise nothing, and the
event stays where it is - the counterpart of `try_recv` with a barrier. Recommended instead of
sending to itself.

### The external agent

An external agent's work is one exchange with its remote agent (`ExternalAgentActor`); what it
takes in during that exchange it could pass on only by a second call into a task the remote agent
is working on, which the A2A protocol allows only where the task waits for input. Out of scope;
its `take_in` between exchanges stays as it is.

### Related issues

- [222](222-queued-requests-answered-one-by-one.md): the rules for what is queued; this issue
  extends them into the turn, and keeps the authority rule decided there.
- [223](223-full-inbox-drops-events.md) and [385](385-inbox-lost-on-restart.md): the planned
  `InboxReceiver` has to offer `try_recv_if` as well, and what is taken in during a turn has to be
  committed only after the state that holds it is stored, as between turns - the run loop stores
  after every step, the commit follows that store.
- [234](234-modify-revoke-and-retry-messages.md),
  [235](235-retry-of-an-answer-under-way.md): answering again stays a barrier; a turn asked for
  again during its run is interrupted by its control (`Interrupt::Retry`) as today.

## Decisions for the maintainer

1. Where: A, before every model call (recommended), or more (B)?
2. Whose words: (ii) a barrier at another initiator (recommended), or (i) hear everything?
3. Changes to messages: barriers at first (recommended), or taken in from the start?
4. Sending to itself replaced by `EventReceiver::try_recv_if` - leaving in the inbox what waits
   (recommended)?

## Estimate

With the recommendations, five steps, each reviewable alone; about the size of 222. No change to
the model's stored data, no change to the moderator or the channel.

1. **`EventReceiver::try_recv_if`** (`caelumora-runtime/src/transport`), `InMemoryEventReceiver`
   and its tests - small, no change of behaviour.
2. **Taking in during a turn** (`actor/intake.rs`): the per-event part of `take_in` shared, and a
   second entry that takes the prefix of the queue up to the first barrier - by kind, by
   initiator, by classification - perceives and notes as `take_in` does, and drops the requests.
   Unit tests next to those of `take_in`. Small to medium; the care is in the barrier predicate.
3. **The actor** (`actor/agent.rs`): `handle_planning` takes in after `may_call_model` and before
   `compaction_due`; the run loop commits after it stored the step. Small.
4. **Tests.** In `channel_tests` with a scripted model: a message said while a tool runs is in the
   context of the next call, and no second turn follows its request; a message of another user is
   not, and is answered in a turn of its own afterwards; a command during a turn is carried out
   after it, and the message behind it heard after it; a message above the turn's classification
   waits. In the agents' security suite: a turn does not hear, between its steps, the words of
   another initiator. Medium - the largest part.
5. **Documents.** AGENT-TURN.md, *In a turn* (`Planning` takes in what was said) and *Between
   turns*; CHANNELS.md where the inbox is described; the doc comments of `Transcript::modify` and
   `AgentChannelState::perceive`. Small.

Risk: low to medium. The model side exists; what is new is a decision in the actor, and its
security rule has to be right - hence the barrier rather than a rule that reorders.

## Done when

- An agent in a turn of several steps takes into account, from its next model call on, what
  others it may hear said in the meantime; it does not answer it again afterwards.
- What it may not take in during a turn - by kind, by authority, by classification - is taken in
  after the turn as today, in order.
- Tests show both, the security suite the rule of whose words a turn hears.
- AGENT-TURN.md describes when a turn takes in.

## History

- **2026-10-19** — Issue created at the maintainer's request: agents should take in what other
  participants say between the steps of a turn, and the question how to deal with events that are
  no messages was raised, with sending them to itself as one option. Analysed in the code: the
  model already takes in during a turn (`perceive`, issue 260), the actor reads its inbox only
  between turns and for approvals. Proposed: taking in before every model call, the prefix of the
  queue up to the first event that has to wait; requests absorbed; the words of another initiator
  and messages above the turn's classification waiting for the turn's end; leaving events in the
  inbox (`try_recv_if`) rather than sending them to itself. Decisions open (above).
