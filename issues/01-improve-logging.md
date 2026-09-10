# 01 — Improve logging

**Status:** open
**Area:** Observability

## The problem

Logging in the workspace is neither consistent nor sufficient.

Two logging frontends are in use at the same time: `caelumora-runtime`,
`caelumora-model` and `caelumora-tui` depend on `log`, while
`caelumora-rest-server`, `caelumora-client-api`, `caelumora-local-client` and
`caelumora-rest-client` depend on `tracing`/`tracing-subscriber`. Whichever
subscriber a binary installs, half the workspace's output can end up in a
different sink or be dropped entirely.

Coverage is thin and uneven as well. `JsonFileStore` and `AgentActor` account
for most of the log statements in the tree; the REST handlers, the
`SessionLoop`, the `EventBus` and the component container are nearly silent.
When a request produces no answer, there is currently no trace to follow from
the HTTP handler through the channel loop into the agent's cognitive cycle.

## Why it matters

The system is asynchronous and actor-based. A failure is almost never visible
at the place where it surfaces — a message that never arrives may have been
dropped by the moderator, queued behind a pending approval, or lost with a
dead actor. Without a log that spans those hops, every such report becomes a
code-reading exercise.

## Considerations

- Settle on **one** frontend for the whole workspace. `tracing` is the
  stronger candidate: it has spans, which fit an actor system where a single
  logical operation crosses several tasks, and `tracing-log` can absorb
  records from any dependency that still emits `log`.
- The natural span boundaries are: an incoming REST request, a `ChannelLoop`
  envelope dispatch, and one turn of the agent cognitive cycle. Session,
  channel and actor ids belong on the span, not repeated in every message.
- Log levels need a convention: `error!` only for things that abort an
  operation, `warn!` for degraded-but-continuing (an MCP server that failed
  discovery, an approval that timed out), `info!` for lifecycle transitions,
  `debug!` for per-event detail.

## Done when

- One logging frontend is used across all crates.
- Session/channel/agent ids can be followed through a single request without
  reading code.
- `AGENTS.md` states the convention (it currently prescribes `log`).

## History

- **2026-09-10** — Issue created from `TODO.md`.
