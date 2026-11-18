# 592 — An event's, a span's and a correlation's ids are bare UUIDs

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `EventContext` holds `event_id`, `correlation_id` and `span_id` as plain `Uuid`, and so do the places that hand them on - a span returned by a user's actor, the span of a command, what the REST API answers -, so that the compiler does not tell one from another, nor from any other id.

## The problem

Every other identity has a type of its own (`define_id!`): `SessionId`, `ChannelId`, `MessageId`,
`TurnId`, `ThreadId`. The three ids that follow an envelope through the system do not:
`EventContext` (`caelumora-domain/src/actor/envelope.rs`) holds `event_id`, `correlation_id` and
`span_id` as `Uuid`. So do the places they travel through: `ChannelAnswering::with_span(Uuid)`,
the `UserActor`'s operations, which answer the span they started as `Result<Uuid>`,
`CommandContext::span_id: Option<Uuid>`, the logging spans (`caelumora_api::logging`), the REST
API's types and the terminal program. About 34 signatures and fields in all.

## Why it matters

A span passed where a correlation is meant, or an event's id where a message's is, compiles. The
three differ in meaning - an event is one envelope, a correlation joins a request with what
answers it, a span joins what one action of a user caused - and a reader learns which one a
`Uuid` is from its name alone. A type of its own makes the compiler check it, as it does for every
other id.

## Considerations

- **`EventId`, `CorrelationId`, `SpanId`** by `define_id!`, as the other ids are, in
  `caelumora-domain/src/actor` beside `EventContext`. Serialized as the UUID they wrap, so that
  nothing stored or answered changes its form.
- **Where they reach**: the API's and the kernel's signatures, the logging spans, the REST API and
  the web UI's types, the terminal. A change without behaviour.
- **Whether a correlation and a span are one concept**: decided before the types are made, so that
  two types are not made for one.

## Done when

- `EventContext` and every place that hands its ids on hold `EventId`, `CorrelationId` and
  `SpanId`, or the types that a decision on their concepts leaves; no bare `Uuid` names one of
  them.

## History

- **2026-11-17** — Opened from the work on issue
  [590](590-a-session-lets-nobody-go.md), as the maintainer asked: ids as types of their own for
  type safety at compile time, apart from the actor ids in REST paths, which 590 takes up.
- **2026-11-18** — Done, by the maintainer's decision for two types: a correlation is no
  identity of its own but the `EventId` of the event answered (`EventContext::next`,
  `ChannelAnswering::replying_to`), and a span is what one initial event caused. `EventId` and
  `SpanId` by `define_id!` beside `EventContext`; `correlation_id` is an `EventId`. The
  `UserActor`'s operations answer `SpanId`, `with_span`, `CommandReply` and `CommandReplyResponse`
  take it; the form stored and answered is the UUID as before, held by a test. The web UI holds
  every id as a `string` and is unchanged. Left open: the nil `EventId` for what answers nothing,
  and the name `correlation_id` for what names the event answered.
