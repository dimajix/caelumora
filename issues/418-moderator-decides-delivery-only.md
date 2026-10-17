# 418 — The moderator returns envelopes, so it could change what a participant is told

**Status:** done
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

Beside that, the rule *how every channel routes* - a message to every user and to every agent that
hears something in it, a change to everyone but its sender, how an answer comes about to the users
alone, a command to the agent it names, a private answer to its issuer - lies in the moderator
(`ModeratorActors::route_said`, `route_to_users`, `route_to_participants`, `route_to_user`, …),
though no moderator decides it: CHANNELS.md says "the built-in moderator routes the way every
channel routes", and a plugin's moderator would have to copy it.

That stands in the way of [417](417-user-history-from-its-actor.md), which records in the channel's
history whom every entry was delivered to, so that what any participant saw can be read back from
one record: that only holds if what a participant was delivered is what the record holds - the
event as it entered the channel, projected by the channel's one rule (`Message::published`) - and
not something a moderator made of it; and the record stays small only if "delivered as every
channel delivers" is one word, not a list of everyone present.

## Decision

**A moderator decides delivery, never content.** It says whether an event goes where every channel
sends it or to some participants alone, whom to ask to respond, and which of its own notices to
tell whom. The channel owns the rule by which "everybody" is delivered, builds every envelope - in
the event's name for a delivery, in the system's for a notice - and delivers exactly what it
records. Where a channel ever wants a message shortened for some of its participants, that is a
question of confidentiality (a classification, a release) or a fixed, deterministic projection the
channel applies and records - not a moderator rewriting.

## Considerations

### The new answer of `route`

```rust
/// Whom something in a channel goes to (issue 417).
pub enum Audience {
    /// Everyone it concerns, by the rule every channel delivers by
    /// (`ChannelLoop::delivers_to`): a message to every user and every
    /// agent that hears something in it, a change to everyone but its
    /// sender, how an answer comes about to the users, …
    Everybody,
    /// These alone.
    Only(Vec<ActorId>),
}

pub(crate) struct Routing {
    /// Whom `event` is delivered to, as it is - never its sender.
    pub audience: Audience,
    /// Whom to ask to respond to what `event` said, by a
    /// `ResponseRequested` delivered after it.
    pub ask: Vec<ActorId>,
    /// What the moderator says of its own, in the system's name, and to
    /// whom - a chain cut off (issue 200).
    pub notices: Vec<(Audience, Notice)>,
}

pub(crate) enum Notice {
    ChainCutOff { message: MessageId, max_agent_hops: u32, not_asked: Vec<ActorId> },
}
```

`ActorEvent::ChainCutOff` stays as what a user or agent receives; the moderator no longer builds
it. A nudge (`NudgeRequested`, issue 278) is answered by `ask: vec![participant]`. The agent hops
(`EventContext::agent_hops`) the request carries are set by the channel from what the moderator
counted: the moderator answers `agent_hops` with the request, or the channel counts as the
moderator did - to be settled as it is built; the count must not move to a place that does not
decide the chain.

### The rule of "everybody"

`ChannelLoop::delivers_to(&ActorEvent) -> impl Iterator<ActorId>` is what `ModeratorActors` does
today for `Everybody`, moved to the channel, which knows its actors: `route_said` (users, and
agents if `Message::is_heard`), `route_to_participants` (a change), `route_to_users` (how an answer
comes about, a turn given up, where an agent stands), `route_to_everybody` (who joined or left,
also whose name it is told in), the approval and question routes (to the approvers, to the users
asked, to the agent asking), `route_to_agent` and `route_to_user` for what names its recipient
(`CommandRequested { agent }`, `CommandExecuted { to }`, `RetryRequested { author }`). The
moderator keeps what is a decision: whom to ask, when to cut a chain, what to tell about it, and -
for one that wants to - `Only` for a delivery that deviates.

`ModeratorActors` keeps the actors the moderator needs for its decisions (the candidates, the
users for a notice, who is mentioned) and loses the routes.

### What the moderator is given

The envelope as it entered the channel, with `Message::published` applied: what everyone but the
sender may see of it. Today the sender's actor applies that before sending, so the moderator sees
nothing else anyway; 417 keeps it so, and the channel asks nothing of the moderator for a message
of which nothing is left to show. The moderator keeps what it keeps for its policy from that form
(`hear`, issue 414).

### Where the envelope is built

`ChannelLoop::route_event_impl` builds one envelope per delivery from the event as it entered -
`Envelope::forward` for a request, `Answering::in_channel` in the system's name for a notice - and
stamps the contexts (channel, session, span) as today. What was sent to an actor is thereby what
the history can say was sent: the record of 417 is written from the same `Routing`.

### What goes

`RouteDecision` is gone since issue 220; nothing else in the runtime builds envelopes for others.
The moderator tests that match on `ActorEvent::ResponseRequested` in a decision match on `ask`
instead; those that count whom a message reaches test `delivers_to` of the channel.

## Done when

- `Moderator::route` returns a `Routing`; no moderator builds an envelope, and no moderator holds
  the rule by which every channel delivers.
- The channel builds every envelope it delivers from the `Routing`, and the tests of
  `policy_moderator` and of the policies assert on the routing.
- CHANNELS.md says that a moderator decides delivery and never content, and where the rule of
  "everybody" lives.
- No behaviour changes: what every participant receives is what it received before.

## History

- 2026-10-17: Opened as the step before [417](417-user-history-from-its-actor.md): a history that
  records whom every entry went to needs the guarantee that what went is what the record holds.
- 2026-10-17: `Audience::Everybody | Only` instead of a list of recipients, and the rule of
  "everybody" moved to the channel, so that 417's entries stay small and a plugin's moderator need
  not copy how every channel routes.
- 2026-10-17: Done on `issue-418-moderator-routing`. `Moderator::route` returns a `Routing`
  (`deliver_to`, `ask`, `agent_hops`, `notices`); the channel builds every envelope
  (`ChannelLoop::deliveries`). Three things differ from the text above:
  - The rule of "everybody" is not `ChannelLoop::delivers_to` but `Envelope::concerns(participant)`
    in `caelumora-model`: a rule over the event, its sender and a participant, needing nothing of
    the runtime - and the predicate by which 417 reads a participant's view of the history needs
    the same rule. Its tests replace the moderator's tests of whom what reaches.
  - No `Audience` yet: `deliver_to: Option<Vec<ActorId>>`, `None` for the rule. Nothing in the
    runtime delivers to some alone but the channel's own tests, so the enum would have been dead
    code; it comes with 417, where the history records it.
  - A notice names its recipients as a list: "everybody" has no meaning for a notice the rule does
    not know.
  The hops of a request are the moderator's to count (`Routing::agent_hops`), as the chain is its
  to decide.
