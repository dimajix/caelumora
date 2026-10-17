# 352 — The approver and the channel build their envelopes by hand beside `Answering`

**Status:** done
**Area:** Code structure
**Epic:** [349](349-core-review-epic.md)
**Summary:** The approver and the channel build their envelopes by hand beside `Answering`, the one way an actor publishes.

## The problem

[211](211-turn-and-agent-work-structure.md) made `Answering` (`actor/mod.rs:140`) the one way an
actor publishes: the contexts of a request continued in its name (`Answering::to`), or its own
(`Answering::own`), and `send` stripping what only the owner sees. Two senders still do it by hand:

- `ApproverActor` has `speaking` (`actor/approval.rs:69`), which continues a security context in
  its own way, and builds `Envelope::new(…)` with `context.next(self.id, self.name())` three times
  (`:186`, `:230`, `:319`). Its answers go through no `published()`, which is right for approvals
  but is not said anywhere.
- `ChannelLoop` builds system envelopes for a refusal (`channel/run.rs:358`) and a removal
  (`:153`), each with `EventContext::new(session, channel, ActorId::System, …)` and
  `SecurityContext::system()`, the refusal copying `span_id` and `correlation_id` by hand.
- `UserActor::envelope` (`actor/user.rs:109`) is the user's own `Answering::own` with a `span_id`.

## Why it matters

Whose name an envelope carries, and what it continues of the request, is what the channel checks
at the door (`OriginEventSender`, `admission`); it should be built in one place, so that a change -
the roles taken anew ([304](304-review-of-the-permission-concept.md), the owner answered for
([341](341-identity-ownership-delegation.md)) - reaches every sender.

## Considerations

- `ApproverActor` keeps an `Answering` per pending approval instead of its two contexts, built with
  `Answering::to(&request, self.id(), self.name(), &own)` where `own` is the approver's context
  (`SecurityContext::new(self.id, session, channel)` answered for by the channel's owner); `send`
  is used as the other actors use it. The `published()` step is a no-op for approvals.
- `Answering::system(session, channel)` for what the channel says itself, and a way to continue the
  span of what is answered (`Answering::to` does it for a request; the refusal needs the same for
  an event that is not one).
- `UserActor::envelope` becomes `Answering::own(…)` with the `span_id` set after, or `Answering`
  gets a `with_span(span_id)`.

## Done when

- Every envelope the runtime sends is built through `Answering`, or this issue says why one is not.
- The approver's and the channel's envelopes carry what they carry today; the approval and
  rejection tests pass unchanged.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
- **2026-10-14** — Done, along the considerations, with two differences:
  - The channel speaks through one constructor, `Answering::in_channel(session, channel, name,
    security)`, rather than an `Answering::system`: the removal is said in the name of who removed,
    whose context may name another place or none (a REST call), so the channel's place is given and
    the sender is the context's caller - `SecurityContext::system()` for a refusal. `replying_to`
    continues the span of the refused event and answers it, `with_span` serves the user.
    `Answering::envelope` builds an envelope without `published()`, for the user and the channel,
    whose events carry nothing for an owner alone and which hand them on otherwise than through an
    `EventSender`; `send` uses it.
  - The approver keeps the request's envelope per pending approval, not an `Answering`, and
    continues it with `Answering::to` as it answers, in its own `security_context()`, answered for
    by the channel's owner. A kept `Answering` would read the owner once instead of at each answer.
    Its answers now go through `published()`, which leaves an approval as it is.

  One behaviour change, asked for by the maintainer: every event of an agent's turn carried the same
  `event_id`, since the turn's `Answering` held the contexts the turn began with. Now
  `Answering::envelope` gives every envelope an `event_id` of its own, keeping the span and what it
  answers (`correlation_id`); `every_message_of_a_turn_carries_the_turn_its_start_announced` shows
  it for a turn. Otherwise no envelope carries anything other than before; the approval and
  rejection tests pass unchanged, and tests of `in_channel`, `replying_to` and `envelope` are new.
