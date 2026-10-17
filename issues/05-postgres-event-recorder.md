# 05 — Postgres event recorder

**Status:** open
**Type:** question
**Priority:** medium
**Area:** Audit
**Summary:** The `Recorder` hook is wired into every channel, but `BlackholeRecorder` is the only implementation, so nothing is recorded.

## The problem

`ChannelLoop` records every envelope it sends and receives through the `Recorder` trait
(`caelumora-runtime/src/recorder.rs`). The only implementation is `BlackholeRecorder`, which
discards both. In other words, the hook for a full event log exists and is wired into the channel,
but nothing is written anywhere.

## Why it matters

The envelope stream is the ground truth of what happened in a session: who said what, which tool was
called, who approved it, in what order. Reconstructing that from agent state after the fact is lossy
— state records the outcome, not the sequence.

Three separate things want this record: debugging (see [01](01-improve-logging.md), which is about
human-readable traces, not this machine-readable one), audit (see [07](07-session-deletion-audit.md)
and [14](14-approval-privilege.md)), and any future replay or "what would this agent have done"
tooling.

## Considerations

- `Recorder` takes `&mut self` and is held under a `Mutex` in the channel; a database-backed
  implementation must not make the channel loop wait on a round trip. Buffer and write
  asynchronously, and decide explicitly what happens when the buffer fills — dropping records
  silently would defeat the purpose for the audit use case.
- Envelopes carry the `SecurityContext`, which is exactly what audit wants and also means the
  recorded stream is sensitive. Retention and access control are part of this issue, not an
  afterthought.
- An event log is naturally append-only and partitioned by time; this is the one store where the
  schema is genuinely simple.

## Done when

- A Postgres recorder persists sent and received envelopes without blocking the channel loop.
- The behaviour under back-pressure is defined and documented.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-10-17** — [417](417-user-history-from-its-actor.md) makes the channel's history the
  record this issue asked for, for a channel: every envelope that enters, every notice and request
  the channel sends and every refusal is an entry, with its audience and its position; only what
  streams and where an actor stands is not. The `Recorder` hook goes with it. What remains of this
  issue - a record across channels, retention, who may read it - is [140](140-audit-log.md), the
  audit log of acts without content; the history is one of its sources. To be closed once 417 is
  done.
