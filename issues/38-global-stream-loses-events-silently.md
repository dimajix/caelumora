# 38 — The global event stream loses events without telling the client

**Status:** done
**Area:** Sessions and channels

## The problem

`GET /events` forwards the `EventBus`'s `subscribe_all` receiver, an
`mpsc` channel with room for 100 events. When it is full, `EventBus::publish`
drops the event for that subscriber and logs a warning; the subscriber is
not told. A client that falls behind — a slow connection, a tab the browser
throttles — therefore misses events and has no way to notice.

The channel stream handles the same situation: it reads a `broadcast`
channel, whose receiver reports a lag, and a client that asked for
snapshots is sent a fresh snapshot instead (issue 34).

## Why it matters

Since [35](35-sidebar-live-updates.md) the web UI's session list is built
from this stream alone. A missed `SessionDeleted` leaves a session in the
list that no longer exists; a missed `SessionCreated` hides one that does.
Both stay wrong until a reload, and nothing hints that a reload is needed.

## Considerations

- **Same answer as for channels.** Give the global stream a way to detect
  loss and answer it with a fresh snapshot when the client asked for one.
  The client already replaces its whole state on every snapshot.
- **How to detect it.** Either switch `subscribe_all` (or just the stream's
  subscription) to a `broadcast` channel, whose receiver reports
  `Lagged(n)`, or keep `mpsc` and mark the subscriber as lagging when a
  send fails, so the stream can emit a snapshot before the next event.
  The first matches the channel stream; the second keeps the bus's other
  subscribers as they are.
- **Buffer size** is a separate knob: a larger buffer makes loss rarer but
  does not make it visible.

## Done when

- A client of the global stream that asked for snapshots and lost events
  receives a fresh snapshot instead of silently continuing.

## History

- **2026-09-24** — Opened while implementing [35](35-sidebar-live-updates.md),
  which made the web UI depend on this stream for its session list.
- **2026-09-24** — Done with the first option, so that both streams work
  the same way. Every `EventBus` subscription is a `broadcast` channel of its
  own (capacity 128, a power of two because broadcast rounds up to one); a
  subscriber that falls behind loses the oldest events and its receiver gets
  `Lagged`. `EventBus::subscribe` returns an `EventSubscription` - named to
  go with `EventBus` and `EventPublisher`; `EventReceiver` was considered and
  dropped because `transport::EventReceiver` already names the actors'
  envelope receiver, used next to it in the same code. Its `recv` logs a loss
  and goes on, so the runtime's own subscribers (`SessionLoop`,
  `ChannelLoop`, the actors) changed little; the global REST stream takes the
  raw receiver with `into_receiver`. Both REST streams now answer a loss
  through the shared `sse::resync_on_lag`: a fresh snapshot for a client that
  asked for one, otherwise a comment with the number of lost events. The
  subscriber that is gone is now recognised by the failing broadcast send
  instead of a failing `try_send`.
