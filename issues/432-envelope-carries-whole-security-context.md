# 432 — Every envelope carries its sender's whole security context, to everyone it reaches

**Status:** done
**Type:** refactoring
**Priority:** medium
**Area:** Identity and access management
**Summary:** An envelope carries the whole `SecurityContext` of its sender - owner, principal, roles, limiting roles - to every actor and every frontend it reaches, though a receiver needs only who began the chain, and the channel takes the roles in it on trust.

## The problem

`Envelope` has a field `security: SecurityContext`, set by the sender's actor (`Answering`), and
every envelope the channel delivers carries it on: to every actor it is routed to, and through
the user's actor and the server-sent event stream to every browser showing the channel.

Looked at in the code (2026-10-19), the context is read in two places that want different things:

- **At the channel's door, the sender's context, whole.** `OriginEventSender::speaks_as` reads
  `caller_id` (it has to be the sender); the admission (`ChannelLoop::admission`, `run.rs`) asks the
  `AccessPolicy` and `require_publish` with caller, owner, principal, roles and limiting roles -
  "E" in [PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md); the decision moderator
  (`caelumora-decision`, `DecisionPolicy`) asks its model with it and checks `may_publish` against
  the sender's clearance.
- **At a receiver, almost only `initiator_id`.** `Answering::to` continues a request with
  `SecurityContext::next`, which takes from it the initiator - the root of the chain, "for the
  record, never for rights" -, the session and the channel, and the roles from the receiver's own
  context; the session and the channel are in the `EventContext` already. Whether an agent takes
  in an envelope between the steps of a turn (`HearsDuringTurn::takes`, issue
  [431](431-hearing-during-a-turn.md)) compares initiators. A `CommandExecuted` is
  recorded with the initiator as its issuer (`Envelope::entry`). The one exception: an agent
  carrying out a command (`AgentBody::carry_out`) asks `ExecuteAgentCommand` in the issuer's whole
  context ("E,3") - deliberately, as the owner of the act (rule 4), while the door checks only the
  form. The issuer of a command is a user by that form; caller, owner, principal and initiator are
  that user, so all but the roles follow from the sender.

What follows from that:

- **Redundant fields.** `session_id` and `channel_id` are in the envelope twice; `caller_id` is
  always the `sender_id` (`OriginEventSender`).
- **More than anyone needs reaches the frontends.** Every envelope goes out to the web UI with its
  sender's owner, principal, roles and limiting roles: a user's browser is told the roles of
  another user, or of the owner of an agent. Nobody reads it (`security: unknown` in the web UI's
  types; neither the TUI nor the REST client touch it).
- **Roles taken on trust.** `OriginEventSender` checks only the caller; owner, principal and roles
  are what the sending actor wrote, and the admission and the moderator decide by them. For code
  in the process this is a small risk, and "E" is documented as "as its sender's actor set it" -
  but a plugin's actor could write any roles.
- **It stands in the way of [223](223-full-inbox-drops-events.md).** The design weighed there on
  2026-10-19 builds the envelopes an actor missed back from the channel's history, which records
  no security context. If a live envelope carries more than one built back, an actor behaves
  differently after a restart or a backlog than live - most visibly a command, checked live with
  the roles as they were when it was asked. Recording the whole context in every history entry
  would cost space for fields no receiver reads.

## Considerations

### A - keep the envelope, record the initiator

The history records the initiator, and an envelope built back gets a context made of its
sender's context as it is now and that initiator. Little to change; but live and built-back
envelopes stay different, and the frontends and the trust stay as they are.

### B - the initiator into the `EventContext`, the context out of the envelope (recommended)

- **`EventContext::initiator_id`.** The initiator moves to where the rest of the chain is -
  `correlation_id`, `span_id`, `agent_hops`; `next` keeps it as it keeps those.
- **`Envelope` loses `security`.** It is timestamp, event context and event.
- **The door builds "E" itself:** the sending actor's own context, with the initiator of the
  event context. Every kind of actor has one already (`AgentBody::security_context`,
  `UserActor::security_context`, the approver's); `Actor` gets a method answering it, so that the
  channel asks the `ChannelActor` it knows instead of reading the envelope. The admission and the
  `DecisionPolicy` are handed that context by the channel.
- **A receiver builds what it needs:** a turn's context ("T") is what `Answering::to` builds
  today, from its own context and the request's initiator; an agent carrying out a command builds
  the issuer's context through `UserController::security_context`, with the issuer's roles as they
  are when the command is carried out.
- **The history records the initiator** of an entry (and `agent_hops` with `ResponseRequested`),
  nothing else of a context - in [223](223-full-inbox-drops-events.md), not here.

What it gains: a live envelope and one built back from the history are the same; the frontends
are told nothing of anyone's roles - compatible with the web UI, which ignores the field; the roles
the door decides by are no longer read from the envelope. Honestly: for a plugin's actor the trust
is the same, as it then answers its context through a method of the trait instead of a field.

What it costs: the method on `Actor`, for the external agent and the internal actors too; the
admission and the `DecisionPolicy` taking the context from the channel; the agent needing a way to
the issuer's `UserController`; some 30 calls of `Envelope::new` and the tests that build envelopes;
the documents on "E" (PERMISSION-CHECKS.md, SECURITY.md, CHANNELS.md). No stored data changes:
envelopes are not stored.

### C - the channel keeps a context per participant

The channel would build each participant's context as it takes it in and never ask the actor.
Rejected: the context holds the owner's roles and the personality's limit as they are now
(issue 304), which change while the channel runs; the actor already knows them.

## Decisions for the maintainer

1. B, as preparation for [223](223-full-inbox-drops-events.md) - decided on 2026-10-19.
2. A command waiting in an inbox or built back from the history is checked with the issuer's roles
   as they are when it is carried out, not as they were when it was asked (recommended): whoever
   lost a role meanwhile should not have a command carried out for them that the role allowed.

## Done when

- `Envelope` has no `SecurityContext`; the initiator is part of the `EventContext`.
- The admission, the decision moderator and an agent carrying out a command ask in a context built
  where it is needed, and the security suites state it: the door decides by the sending actor's own
  context, not by what an envelope says; a command is checked with the issuer's roles as they are.
- The web UI's `Envelope` type no longer names `security`.
- PERMISSION-CHECKS.md says what "E" is built from.

## History

- **2026-10-19** — Opened while revising the plan of [223](223-full-inbox-drops-events.md): built
  from the channel's history, an envelope would need its head recorded, and looking at which of
  its security context is read showed that a receiver needs the initiator alone, and that the
  whole context reaches every frontend. The maintainer chose B, to be done before 223.
- **2026-10-19** — Done, not as B. B was built and taken back at the maintainer's word: a context
  the sender gives reflects its rights and ownership when the request was given, a context built
  again those when it is carried out - two times to keep apart, a more complicated model than one.
  So an envelope keeps the `SecurityContext` its sender's actor set, as "E" says, and the checks
  read it as before; actors give theirs at many places anyway. `session_id` and `channel_id` stay
  in both contexts: the `EventContext` says where something happens, the `SecurityContext` where
  it is checked - the same values for very different purposes.

  What is done: the frontends are shown no `SecurityContext`. A channel's stream serializes an
  envelope as a `ShownEnvelope` (`caelumora-rest-api`), its time, its event context and its event;
  the web UI's `Envelope` type no longer names `security`. And the initiator is part of the
  `EventContext` as well (`initiator_id`, the sender by `EventContext::new`, kept by `next`, set
  from the security context by `Answering::in_channel`), beside the span, its depth and the hops
  of the chain it belongs to: what goes by the chain reads it there - whose authority a turn hears
  between its steps (`HearsDuringTurn::takes`, comparing the work's event context), who asked for
  a command carried out (`HistoryEntry::CommandExecuted::issuer`) -, what checks rights the
  security context.

  For [223](223-full-inbox-drops-events.md): the history is the one place a security context is
  built again - for an envelope built back from it when an actor joins or its channel starts again
  after a pause. It records the initiator, which cannot be built again; the rest comes from the
  sender as it is then, so that a request taken up after a pause is checked with the rights of
  then - the one deliberate exception, to be stated there. Stored data: an agent's channel state
  stored while a turn was under way holds the turn's `EventContext`, which now needs
  `initiator_id` - such states are to be deleted.
