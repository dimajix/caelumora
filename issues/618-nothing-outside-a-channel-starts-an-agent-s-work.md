# 618 — Nothing outside a channel starts an agent's work

**Status:** open
**Type:** question
**Priority:** medium
**Area:** Sessions and channels
**Summary:** An agent's work begins only with something said or given in a channel by a participant or a delegate; an event outside every channel - an appointment falling due, a mail arriving, a message on a messenger - has no way to become an agent's work, and nothing says who would bring it in, where it lands, in whose name it runs and what may flow.

## The problem

Every turn of an agent begins because something reached it in a channel: a message of a
participant, a request to respond, a brief given by the delegate of a thread (issue
[557](557-a-conversation-cannot-go-on-two-ways-at-once.md)), a command. Two kinds of work begin
with nobody speaking, and neither is meant for events from outside:

- an external agent's task that outlives its request speaks on its own when it ends
  ([199](199-external-agent-speak-wakes-all.md), CHANNELS.md *External agents*);
- work interrupted by a restart is resumed (`procedure::session::resume_interrupted`, issue 388).

Three issues need the same thing - an event outside every channel that becomes an agent's work:

- [612](612-an-agent-cannot-keep-an-appointment.md): an appointment falls due - the event is the
  kernel's own, at a time;
- [613](613-an-agent-cannot-read-or-send-email.md): a mail arrives - from outside, untrusted, not a
  conversation of a channel;
- [614](614-no-messenger-reaches-a-channel.md): a message arrives on a messenger - from outside, and
  a conversation, which a channel is.

612 first described what the three share (*What the three share*); this issue holds the decision,
so that the three are built on one answer, not three.

## Why it matters

- Without it each of 612–614 invents its own way into a channel - three internal actors, three ways
  of choosing an authority, three rules of what may flow -, which is what AGENTS.md calls a variant
  beside a pattern.
- The company of the epic [617](617-epic-a-company-that-works-on-its-own.md) works largely on
  events nobody in a channel said: time, mail, customers.

## The questions

### 1. Who brings it in

The closest pattern is the delegate of a thread (`caelumora-kernel/src/actor/delegate.rs`,
CHANNELS.md *Delegating into a thread*): an internal actor of the channel, no participant, which no
configuration names (`Channel::is_internal`), gives its recipient a text to read (`GivenToRead`)
and asks it alone to respond (`RequestReason::Given`); the recipient takes it in as what it was
given, recorded once (`MessageGiven`), and the other agents do not hear it.

- **A. One internal actor per source** - a calendar's, a mailbox's, a messenger's bridge -, each
  built on the delegate's way of giving. Each source keeps its own state where it belongs; the
  channel needs to know each kind.
- **B. One internal actor for whatever comes from outside**, which the sources hand their events to
  through the kernel (a service, say, that a plugin calls with "give this text to that agent in that
  channel, in this name"). The channel knows one more internal actor, the sources none of the
  channel.
- **C. The kernel gives it through the channel directly**, without an actor - as the channel's
  notices are told. Fewest parts; but what is given then has no sender the history can name, and
  every other giving in a channel has one.

To weigh: a messenger's bridge (614) carries a conversation both ways and is more than a giver, so
it may be an actor of its own whatever is chosen for the one-way sources.

The concept needs one name - the event, or the giving of it -; *trigger* is a proposal.

### 2. Where it lands

In a channel that runs:

- the channel the event was made in - a reminder set there;
- a channel the source is bound to - a mailbox's mail room (613), a messenger chat's channel (614);
- a session made for it from a template the source names - an appointment that starts work of its
  own, a chat seen for the first time.

A session that is not running is started for it, as `resume_interrupted` starts the sessions with
interrupted work. Open: whether a channel at all is needed for one agent working alone, or whether
a session of one agent made from a template is that channel - recommended, so that nothing new is
needed where an agent works.

### 3. In whose name

An agent reads in its owner's name and acts in the name of whoever asked it
([255](255-whose-name-agents-act-in.md)). An event from outside has no one who asked at the moment
it is given. Precedents:

- an external agent's late task speaks with the authority of the request that began it;
- one without that request speaks "afresh, as the agent itself" - its own work, in its owner's
  name (CHANNELS.md *External agents*).

Proposed: an event an agent set up in a turn - an appointment - keeps that turn's initiator, checked
again when it is given, since the initiator may have left the channel; an event from outside - a
mail, a messenger message - is the agent's own work, in its owner's name. Where neither may act in
the channel any more, the event is dropped and its owner told.

### 4. What may flow

- **What the event says.** An appointment's text is classified as the work that wrote it, and is
  given only into a channel it may flow into.
- **What comes from outside** is untrusted: public at most, and marked as coming from outside where
  the model reads it, so that an instruction in a mail is read as the mail's, not the owner's.
- **What goes out** - a reply by mail, a message into a chat - leaves the system: only what may be
  published, as at the door an external agent passes (CONFIDENTIALITY.md).

## Done when

- The four questions are decided and recorded here, with the name of the concept.
- 612, 613 and 614 refer to the decision; the first of them to be built builds the mechanism, in a
  step of its own before its feature.

## History

- **2026-11-27** — Opened on the maintainer's decision to split the foundation that 612 described
  for 612, 613 and 614 into an issue of its own.
