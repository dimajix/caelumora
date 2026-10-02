# 115 — `FilteringEventSender::send` drops silently while `try_send` reports the drop as an error

**Status:** done
**Area:** Maintenance

## The problem

`transport_impl/memory.rs:143-151`: `send()` returns `Ok(())` when the filter rejects an envelope;
`try_send()` returns `Err(Filtered)` for the same case. `ChannelLoop::route_event_impl`
(`channel.rs:702`) then has to recognise `Filtered` as the normal case and swallow it.

## Why it matters

Two methods of one sender disagree on whether a filtered envelope is an error; the caller
compensates. The next caller of `try_send` will not know to.

## Considerations

Both return `Ok(())` for a filtered envelope - filtering is what the type is for. If a caller needs
to know, a `bool` ("was sent") is the honest answer, not an error. Two methods, one call site.

## Done when

`send` and `try_send` treat a filtered envelope the same way, and `route_event_impl` matches no
`Filtered` error.

## History

- **2026-09-28** — Split off from [97](97-small-model-unifications.md) (structure review, 3.2.10).
- **2026-09-29** — Done. `FilteringEventSender::try_send` returns `Ok(())` for an event its filter
  rejects, like `send`; `TransportError::Filtered` had no other use and is gone, and
  `route_event_impl` no longer matches it. A unit test sends a filtered event both ways and checks
  that only what passes the filter arrives. No caller needed to know whether an event was filtered,
  so no `bool` was added.
