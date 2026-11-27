# 626 — Every streamed chunk passes the channel on its own

**Status:** done
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** Every chunk a model streams passes the channel as an envelope of its own - admitted, routed and delivered one by one -, and an agent's turn waits for the channel once ten of them are queued, although the chunks of one message waiting together could be taken as one.

## The problem

A turn shows every chunk its model streams as a partial message of its own (`Turn::show`,
`event_for` in `caelumora-agents/src/turn/mod.rs`): a `PartialMessageGenerated`, or an
`IntermediatePartialGenerated` for a step of a plan. All actors of a channel send into one queue of
ten places (`mpsc::channel(10)` in `ChannelLoop::run`, `caelumora-kernel/src/channel/run.rs`),
which the channel's loop takes one envelope at a time:

- `admission`: the thread looked up, the classification of the message checked (`require_parts`);
- `record_and_route`: the moderator's `route`, `append_history` - which records nothing of a
  partial -, and `route_event` into the inbox of every user.

For a partial each of these steps is done in memory, and cheap. The loop is slow while it handles
something else: a message said is stored at once (`append_history`). Meanwhile the chunks of an
agent that streams fill the ten places, and its turn waits in `send` - it stops reading its model's
stream - until the loop takes them. A second agent streaming at the same time shares the same ten
places.

Behind the channel each chunk stays one:

- an entry in the inbox of each user's actor (`channel::inbox`, 100 places). A transient envelope
  that finds the inbox full is dropped (`Inbox::deliver`): the streamed text has a hole until the
  complete message takes its place;
- an item in the actor's outbox (a `broadcast` of 128, `ActorManager::outbox`), an event of the
  channel's stream (`ChannelFollower`, `caelumora-rest-server/src/routes/channels.rs`), and an
  update of the web UI, which appends `content` and `reasoning` of each partial to the answer under
  way (`caelumora-web-ui/src/conversation.ts`), as the terminal does.

The inboxes of agents are not concerned: partials go to the users alone (`Envelope::recipients`),
and no actor but a user's takes them (`Actor::event_filter`).

No symptom has been reported, and nothing has been measured: neither how long the loop takes for
an event, nor how often a turn waits for the channel.

## Why it matters

- A loop busy with something else slows the stream of every model answering in its channel, and
  the chunks of one agent delay the events of the others.
- What follows the channel - inboxes, outboxes, streams, the browser - pays per chunk, not per
  message.
- An inbox that overflows leaves a hole in the text a user watches stream in; a stream that falls
  behind loses the answer under way, since the snapshot it is sent holds no partials.

## Considerations

- **What can be merged.** A partial is a piece of its message, not the message so far: the
  frontends append it. Two partials can be one if they are of the same kind - both
  `PartialMessageGenerated` or both `IntermediatePartialGenerated` -, of the same context but for
  their `event_id` - sender, thread, initiator, span, `correlation_id` -, and of the same message
  but for `content` and `reasoning`: the same `turn_id`, `classification` and
  `reasoning_visibility`, with no attachments, tool calls or tool results. The classification can
  rise during a turn; across such a change nothing is merged. The merged partial carries the
  `content` and `reasoning` of both, one after the other, and the `event_id` and `timestamp` of the
  first: a partial is neither recorded nor referred to, so nobody misses the second's.
- **Where the rule lives.** In the domain, on what it is about - for instance
  `Envelope::absorb(&mut self, next: Envelope) -> Result<(), Envelope>`, which takes `next` in or
  hands it back; the name is the plan's to settle. A queue that merges only asks it, so that a
  second one merging later adds no second `match` over the kinds of events.
- **Order.** All actors of a channel share its queue. Merging only with the envelope right before
  would merge little while two agents stream at once. A partial is therefore merged with the last
  queued envelope of its sender, as long as nothing of that sender is queued behind it: the order of
  each sender's envelopes holds. Between senders the queue holds only the order they arrived in; a
  later chunk of one moves ahead of an event of another it does not depend on - an agent hears no
  partials, and what it hears during a turn changes only its next call of the model.
- **Admission.** A merged partial is admitted once, in the channel's state as the loop takes it.
  What is checked - thread, classification, sender - is the same for its pieces by the rule above,
  so the outcome is that of each piece. A channel reclassified or a thread closed meanwhile applies
  to all of them, as it applies today to everything waiting in the queue.
- **Merged as the loop takes them (chosen).** The loop keeps its `mpsc`, raised to 100 places - the
  inbox's `CAPACITY` -; having taken an envelope, it takes what is queued behind it (`try_recv`),
  merges the partials by the rule and handles the rest in order. A round then handles up to 100
  chunks at the cost of one, and everything after the channel receives them merged. A turn waits
  only once a single handling takes longer than its model needs for 100 tokens - one to three
  seconds at 30 to 100 tokens a second -, and a loop that slow is a problem of its own, which a
  larger buffer would only hide. It costs no new type and no new concept; the queue holds at most
  100 envelopes and stays bounded, so that an actor flooding the channel is still held back; an
  event queued behind many chunks waits for one handling of them, not one per chunk.
- **Merged as they are put in (not chosen).** The channel's `mpsc` replaced by a queue of its own,
  the counterpart of the inbox - actors to channel where the inbox is channel to actor -: its
  sending half merges into the last queued envelope of the sender, and waits for room only where it
  cannot. A turn would never wait for the channel because of a partial, however slow the loop. It
  needs a new type, with its own waiting for room and its own waking, since tokio has no `mpsc`
  that merges, and one more concept to know. Its gain over the above shows only where a single
  handling takes seconds; it is taken up if a turn is seen waiting for the channel despite it.
- **In the inbox of a user's actor (not part of this).** `Inbox::deliver` could merge by the same
  rule, but little stays there to merge: nothing behind it holds the user's actor back. It shows
  what it takes in its outbox, a `broadcast`, whose `send` never waits (`UserActor::show`): a
  follower too slow loses the oldest items (`Lagged`), the actor goes on. The actor waits only as it
  sends into the channel - a message of its user (`forward`), the answer of a rule of theirs to an
  approval (`answer_by_rule`) -, that is for a busy loop, which meanwhile delivers nothing into its
  inbox either. And once the channel merges, what reaches the inbox is merged already.
- **In the stream (not part of this).** A `broadcast` cannot change what it holds. The
  `ChannelFollower` could merge, as it takes them, the partials already waiting, and send one event
  for them. Each event of the stream carries an envelope's whole `EventContext` - session, channel,
  thread, sender, ids, span -, far more than the few characters of a chunk; merged, a stream writes
  fewer bytes, empties the outbox sooner and falls behind less often. That matters: a stream that
  falls behind is sent a snapshot of the channel (`ChannelFollower::next`), built from its history,
  which holds no partials - the answer under way loses what streamed before, until its complete
  message comes. The merging at the channel already makes the items of the outbox fewer; merging in
  the `ChannelFollower` is a matter of its own, should a stream be seen to fall behind a fast model.
- **Other kinds.** `StateChanged`, of which only the last counts, could be taken by the same
  mechanism; not without a need for it.

## Done when

- The partials of one sender queued together for the channel are handled as one, by a rule of the
  domain held by tests of every case in which it does not apply.
- A test of the channel with a busy loop: a user receives the chunks of one message as one partial
  with the whole text, and what other senders sent in between keeps its order.
- The channel's queue holds 100 envelopes; how it merges is described in `docs/CHANNELS.md`.

## History

- **2026-11-27** — Opened from a discussion with the maintainer: merging as envelopes are put into
  the channel's queue, in the inbox of a user's actor and in the stream were weighed; merging as the
  channel's loop takes them, from a queue of 100, was chosen.
- **2026-11-27** — The options of the inbox and the stream corrected: nothing behind the user's
  actor holds it back, since its outbox never waits; merging in the stream does make it fall behind
  less often, and a stream that falls behind loses the answer under way from its snapshot.
- **2026-11-27** — Done as chosen: `Envelope::absorb` and `Message::append` hold the rule in the
  domain; the channel's loop takes what is queued behind an envelope with it (`taken_together` in
  `caelumora-kernel/src/channel/run.rs`), from a queue of 100 (`INPUT_CAPACITY`); described in
  `docs/CHANNELS.md`. The busy loop of *Done when* is tested as a queue filled before the loop
  takes from it: a test through `ChannelLoop::run` with a moderator that holds the loop could not
  tell when another actor's pieces are all queued, and would depend on timing.
