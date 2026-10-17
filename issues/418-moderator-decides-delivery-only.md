# 418 — The moderator returns envelopes, so it could change what a participant is told

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Sessions and channels
**Summary:** `Moderator::route` returns the envelopes to deliver, so a moderator could alter a message for one recipient or speak in any name, and the channel cannot record what was decided apart from what was sent.

## The problem

`Moderator::route(&Envelope) -> Vec<(ActorId, Envelope)>` answers whom an event is delivered to
*and as what*. The built-in `policy_moderator` uses that freedom in three ways only: it forwards the
event as it is, it turns it into a `ResponseRequested` for whom it asks to respond, and it says
`ChainCutOff` in the name of `ActorId::System` (`cut_off`, issue 200). Nothing else is needed, but
the type allows everything: a moderator from a plugin could hand one user a message with a
sentence removed, and hand another the whole, or send any event in anybody's name.

That stands in the way of [417](417-user-history-from-its-actor.md), which records in the channel's
history whom every entry was delivered to, so that what any participant saw can be read back from
one record: that only holds if what a participant was delivered is what the record holds - the
event as it entered the channel, projected by the channel's one rule (`Message::published`) - and
not something a moderator made of it.

## Decision

**A moderator decides delivery, never content.** It says whom an event goes to, whom to ask to
respond, and which of its own notices to tell whom. The channel builds every envelope, in the
event's name for a delivery, in the system's for a notice, and delivers exactly what it records.
Where a channel ever wants a message shortened for some of its participants, that is a question of
confidentiality (a classification, a release) or a fixed, deterministic projection the channel
applies and records - not a moderator rewriting.

## Considerations

### The new answer of `route`

```rust
pub(crate) struct Routing {
    /// Whom `event` is delivered to, as it is - never its sender.
    pub deliver_to: Vec<ActorId>,
    /// Whom to ask to respond to what `event` said, each with the request
    /// they are asked by - delivered after the event, as a
    /// `ResponseRequested`.
    pub ask: Vec<ActorId>,
    /// What the moderator says of its own, in the system's name, and to
    /// whom - a chain cut off (issue 200).
    pub notices: Vec<(Vec<ActorId>, Notice)>,
}

pub(crate) enum Notice {
    ChainCutOff { message: MessageId, max_agent_hops: u32, not_asked: Vec<ActorId> },
}
```

`ActorEvent::ChainCutOff` stays as what a user or agent receives; the moderator no longer builds
it. A nudge (`NudgeRequested`, issue 278) is answered by `ask: vec![participant]`. The agent hops
(`EventContext::agent_hops`) the request carries are set by the channel from what the moderator
counted: the moderator answers `agent_hops` for the request, or the channel counts as the
moderator did - to be settled as it is built; the count must not move to a place that does not
decide the chain.

### What the moderator is given

The envelope as it entered the channel, with `Message::published` applied: what everyone but the
sender may see of it. Today the sender's actor applies that before sending; with 417 the channel
does, and the moderator - whose `ResponderPolicy` may ask a model elsewhere (`decision` plugin) -
must never see what only an owner sees. The moderator keeps what it keeps for its policy from
that form (`hear`, issue 414).

### Where the envelope is built

`ChannelLoop::route_event_impl` builds one envelope per delivery from the event as it entered -
`Envelope::forward` for a request, `Answering::in_channel` in the system's name for a notice - and
stamps the contexts (channel, session, span) as today. What was sent to an actor is thereby what
the history can say was sent: the record of 417 is written from the same `Routing`.

### What goes

`RouteDecision` is gone since issue 220; nothing else in the runtime builds envelopes for others.
The moderator tests that match on `ActorEvent::ResponseRequested` in a decision match on `ask`
instead.

## Done when

- `Moderator::route` returns a `Routing`; no moderator builds an envelope.
- The channel builds every envelope it delivers, and the tests of `policy_moderator` and of the
  policies assert on the routing.
- The moderator is given the event without what only its sender's owner sees.
- CHANNELS.md says that a moderator decides delivery and never content.
- No behaviour changes: what every participant receives is what it received before.

## History

- 2026-10-17: Opened as the step before [417](417-user-history-from-its-actor.md): a history that
  records whom every entry went to needs the guarantee that what went is what the record holds.
