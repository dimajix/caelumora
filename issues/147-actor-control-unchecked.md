# 147 — A frontend can cancel, wake and stop an actor without a permission check

**Status:** open
**Area:** Code structure

## The problem

`Actor::cancel`, `Actor::wakeup` and `Actor::stop` act on an actor without
checking anything, by design: their callers checked. `ChannelLoop::cancel`
asks `CancelChannel`, `ChannelLoop::wakeup_actor` asks `WakeChannel`,
`ChannelLoop::remove_participant` and `SessionLoop::delete_channel` store
their change under `UpdateChannel`, and only then call the actor.

But `Actor` is a public trait, the three are public methods of it, and
`ChannelLoop::actor_by_id` is public and hands out `Arc<dyn Actor>`. A
frontend can call `channel.actor_by_id(agent)?.stop()` and stop an agent
- give up its turn - without any permission. The REST server does not do
it today; it uses `actor_by_id` for `message_context` and
`given_up_turns`, which check themselves. The rule of AGENTS.md - "an
operation checks its own permission" - holds here by discipline, not by
structure.

## Why it matters

- One slip in a handler, and a user who may only read a channel stops,
  cancels or wakes its agents.
- `stop` gives up a turn since [145](145-ended-turns-leave-a-record.md),
  which is recorded; an unchecked path to it would record a stop nobody
  was permitted to cause.

## Considerations

- **A crate-internal trait for control.** `cancel`, `wakeup` and `stop`
  move into a trait only the runtime sees (e.g. `ActorControl`,
  `pub(crate)`), which the `ChannelLoop` uses; the public `Actor` keeps
  what checks itself or is harmless - `id`, `name`, `state`,
  `message_context`, `given_up_turns`. A frontend then cannot reach the
  control methods at all. Needs the `ChannelLoop` to hold its actors as
  both, or the internal trait to be a supertrait the public one does not
  expose.
- **`actor_by_id` returns a narrower view.** Keeps the trait as it is, but
  every new public accessor would have to remember it.
- Decided in the discussion of 145: none of the three knows its caller
  (who did it is the act's, recorded by [140](140-audit-log.md)); whatever
  is done here keeps them symmetric.

## Done when

- A frontend cannot cancel, wake or stop an actor other than through an
  operation that checks its permission.

## History

- **2026-09-30** — Opened in the discussion of
  [145](145-ended-turns-leave-a-record.md): the maintainer asked why
  stopping needs no permission. It needs none as the consequence of an act
  that was checked, but `actor_by_id` lets a frontend call it directly.
