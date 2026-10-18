# 422 — The channel loop bounds routing depth, a decision that belongs to the moderator

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Sessions and channels
**Summary:** `ChannelLoop::decide` refuses to ask the moderator about an event once its `span_depth` reaches a hard-coded `max_route_decisions`, so the channel keeps a routing rule of its own beside the moderator, which decides everything else about routing.

## The problem

Since [418](418-moderator-decides-delivery-only.md) the moderator decides whom an event goes to,
whom to ask to respond and what to tell; the channel builds and delivers what was decided. One
routing rule is still the channel's: `ChannelLoop::decide` (`caelumora-runtime/src/channel/run.rs`)
checks `event.context.span_depth >= self.max_route_decisions` before it asks the moderator, and
answers `None` past it - with a `warn!`, nothing else. `max_route_decisions` is a field of
`ChannelLoop`, hard-coded to 100, not configurable.

[45](45-agent-mentions.md) bounded the chains between agents where they arise: `PolicyModerator`
counts `agent_hops` and asks nobody beyond its parameter `max_agent_hops`, telling so by a
`ChainCutOff` ([200](200-chain-limit-hint.md)). `max_route_decisions` was kept then "as a
safety net against faulty routing". So two bounds of the same kind live in two places: one
configurable per moderator, one fixed in the channel.

What the channel's bound does is also coarser than what the moderator's does: past it, the event is
recorded with an empty audience and delivered to nobody - a message said that deep is swallowed
silently - where the moderator's bound delivers what was said and only asks nobody to respond.

## Considerations

- **Move it into the moderator.** A parameter of `policy_moderator` beside `max_agent_hops`
  (e.g. `max_route_decisions`, default 100), checked in `PolicyModerator::route`: past it, nobody is
  asked, what was said is still delivered. `ChannelLoop::decide` asks the moderator always, and
  `max_route_decisions` leaves `ChannelLoop`. The channel then has no routing rule of its own.
- **What it costs:** the net under every moderator goes. Since 418 a moderator can make further
  events arise only by `ask`, so one that bounds its asks bounds the chain; a plugin's moderator has
  to bound its own. To be checked before: whether anything other than an `ask` makes an actor send
  in reaction to what it was delivered (the approver, an external agent), which only the channel's
  bound covers today.
- **Drop it instead.** In `PolicyModerator`, `agent_hops` bounds every chain it asks for, and with
  the default `max_agent_hops` of 3 the span depth never comes near 100. A second bound there only
  matters for a large `max_agent_hops`. Simpler, but leaves a moderator without any bound on
  routing that is not a chain between agents.
- **Keep it in the channel**, configurable and as "ask nobody" rather than "deliver to nobody".
  Keeps the net for every moderator, but the channel keeps deciding something about routing.

Proposed: move it into the moderator, past the limit asking nobody instead of swallowing the event.

## Done when

- `ChannelLoop` no longer checks `span_depth`; `decide` always asks the moderator.
- The bound is a parameter of the moderator component, and an event past it is still delivered.
- [docs/CHANNELS.md](../docs/CHANNELS.md) says where routing is bounded.

## History

- **2026-10-18** — Opened.
- **2026-10-18** — Done as proposed: moved into the moderator. Checked before: nothing but an ask
  makes an actor send in reaction to what it was delivered. A turn speaks on in the contexts of
  the request it answers (`Answering::continuing`), so its span depth does not grow with its
  steps or the approvals it waits for; an approval and a question are answered once. The
  `policy_moderator` takes `max_span_depth` (default `DEFAULT_MAX_SPAN_DEPTH`, 100; set by
  `PolicyModerator::with_max_span_depth`) and, for an event as deep, asks nobody - mentioned or
  chosen - and tells nothing; what was said is delivered. Named after what it bounds rather than
  `max_route_decisions`. `ChannelLoop` lost `max_route_decisions` and `decide`: `enter` asks the
  moderator about every event it shows. CHANNELS.md and COMPONENTS.md say where routing is bounded.
