# 228 — The channel takes an actor's word for who sent an envelope

**Status:** done
**Area:** Identity and access management

## The problem

Every actor of a channel writes into one shared output, and each sets the sender of its envelopes
itself: `EventContext::sender_id` and the envelope's `SecurityContext`
(`ChannelLoop::spawn_actor`, `UserActor::envelope`). The channel checks only that the sender is
still running (`handle_actor_event`); it does not check that the sender is the actor that wrote the
envelope.

Decisions are made on both: the moderator routes approvals by `sender_id`, so that only users'
answers reach the approver and only the approver's decisions the agents; the approver counts an
answer as the user's whose context it carries ([14](14-approval-privilege.md)); `may_publish` is
asked with the envelope's context and sender. An actor that wrote someone else's id or context into
an envelope would be taken for them.

Today every actor is code of the runtime, so nothing writes a wrong sender. But the guarantee rests
on each actor's implementation, not on the structure: a new actor kind, or an external agent's
actor building its envelopes from what a remote agent sent, is one mistake away from acting in
someone else's name.

## Why it matters

The approver is a policy enforcement point (issue 14) and trusts the context an answer comes with.
An enforcement point is only as good as the identity it is given; that identity should be
guaranteed by the transport, as "actors send only through `run`'s transport" already is.

## Considerations

- **Stamp at the transport.** The `EventSender` handed to an actor in `spawn_actor` knows the
  actor's id: it could set `sender_id` itself, or refuse an envelope with another one. The
  `SecurityContext` is harder: its initiator may legitimately be someone else - an agent acting for
  whoever asked it (issue 211) - so only its `caller_id` can be checked against the actor.
- **Check at the channel.** `handle_actor_event` could compare the envelope with the actor it came
  from, if the shared output carried that, e.g. as a pair of actor id and envelope.
- Forwarded envelopes (`Envelope::forward`, the moderator's notices sent as `System`) must still be
  possible for the channel itself, not for actors.

## Done when

- An envelope that enters the channel from an actor carries that actor as its sender and as the
  caller of its security context, enforced by the channel or its transport, not by the actor.

## History

- **2026-10-05** — Opened while doing [14](14-approval-privilege.md): the approver enforces who may
  approve by the context an answer comes with, and nothing guarantees that context belongs to the
  sender.
- **2026-10-05** — Done, decided with the maintainer: each actor is given an `OriginEventSender`
  (`ChannelLoop::spawn_actor`, the only place an actor gets one), which passes on only an envelope
  that speaks as the actor - its sender, the caller of its `SecurityContext` and the sender of every
  message it carries (`OriginEventSender::speaks_as`, with `ActorEvent::messages`, which
  `ChannelLoop::admits` uses too). Anything else is refused with `TransportError::Refused` and
  logged with `error!`: today it can only be a bug, and the actor learns of it where it happened.
  Checking in the channel instead, with the origin carried next to each envelope, was built first
  and given up: it needed a pair type through the channel's output for no gain, since no envelope
  reaches that output but through an actor's sender. Overwriting the sender was rejected, since it
  would turn a false claim into a true one, and so was stopping the actor. The initiator is not
  checked: an agent acts on behalf of whoever asked it. All actors kept the convention already;
  three tests did not - a test actor without a context and stored tasks of the external agent under
  a made-up agent id - and were corrected.
