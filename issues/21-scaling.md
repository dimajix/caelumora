# 21 — Scaling

**Status:** open
**Area:** Open architecture questions

## The question

The runtime is a single process. `SessionManager` holds live `SessionLoop`s, each holding
`ChannelLoop`s, each holding actors as tokio tasks; controllers keep live state in `ArcSwap` in that
process's memory; the `EventBus` distributes events within it. Nothing in the design is distributed,
and the current stores (`MemoryStore`, `JsonFileStore`) could not support a second instance even if
it were.

The question is what "more load" is supposed to mean here, and it has at least three distinct
answers that pull in different directions.

## Why it matters

Answering it late is expensive: the actor model, the controller/`ArcSwap` state design and the
`EventBus` are all shaped by the single-process assumption, and each would need reworking for a
different answer. Answering it *early* is also expensive, in the ordinary way of building for load
that never arrives.

The point of recording it now is to know which cheap decisions preserve the option — for example the
store work in [03](03-postgres-entity-store.md) and [04](04-postgres-state-store.md), which removes
the hardest blocker as a side effect of solving a problem that exists today.

## Considerations

- Three different scaling axes, which should not be conflated:
  - **More concurrent sessions** — the most likely first constraint, and the one shared stores plus
    a session-to-instance assignment would address. Sessions are largely independent, so this is
    close to embarrassingly parallel.
  - **Larger single sessions** — many agents in one channel. Bounded by the `ChannelLoop`, which is
    a single routing point, and by [16](16-context-compression.md) long before it is bounded by
    hardware.
  - **Throughput per agent** — dominated by LLM latency, not by the runtime. Almost certainly not
    where the effort belongs.
- If sessions are ever distributed across instances, ownership must be exclusive: two runtimes
  driving the same `AgentActor` would corrupt state regardless of what the store guarantees. Some
  form of lease or single-writer claim is the minimum.
- The `EventBus` is process-local. Anything distributed needs it to be backed by something external,
  and every current subscriber (REST SSE endpoints, the TUI) assumes local delivery.
- Interacts with [20](20-transactional-effects.md): most of the interesting distributed failure
  modes are the transactional ones.

## Done when

- The scaling axis that actually matters is identified and recorded here.
- The decisions that would foreclose it are known, so they can be avoided cheaply in the meantime.

## History

- **2026-09-10** — Issue created from `TODO.md`, where it appears under *"BIG questions"*.
