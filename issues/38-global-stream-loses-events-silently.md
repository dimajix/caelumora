# 38 — The global event stream loses events without telling the client

**Status:** open
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
