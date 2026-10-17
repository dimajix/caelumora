# 01 — Improve logging

**Status:** done
**Area:** Observability and operations
**Summary:** Two logging frontends (`log` and `tracing`) are used side by side, and the REST handlers, session loop and event bus are nearly silent.

## The problem

Logging in the workspace is neither consistent nor sufficient.

Two logging frontends are in use at the same time: `caelumora-runtime`, `caelumora-model` and
`caelumora-tui` depend on `log`, while `caelumora-rest-server`, `caelumora-client-api`,
`caelumora-local-client` and `caelumora-rest-client` depend on `tracing`/`tracing-subscriber`.
Whichever subscriber a binary installs, half the workspace's output can end up in a different sink
or be dropped entirely.

Coverage is thin and uneven as well. `JsonFileStore` and `AgentActor` account for most of the log
statements in the tree; the REST handlers, the `SessionLoop`, the `EventBus` and the component
container are nearly silent. When a request produces no answer, there is currently no trace to
follow from the HTTP handler through the channel loop into the agent's cognitive cycle.

## Why it matters

The system is asynchronous and actor-based. A failure is almost never visible at the place where it
surfaces — a message that never arrives may have been dropped by the moderator, queued behind a
pending approval, or lost with a dead actor. Without a log that spans those hops, every such report
becomes a code-reading exercise.

## Considerations

- Settle on **one** frontend for the whole workspace. `tracing` is the stronger candidate: it has
  spans, which fit an actor system where a single logical operation crosses several tasks, and
  `tracing-log` can absorb records from any dependency that still emits `log`.
- The natural span boundaries are: an incoming REST request, a `ChannelLoop` envelope dispatch, and
  one turn of the agent cognitive cycle. Session, channel and actor ids belong on the span, not
  repeated in every message.
- Log levels need a convention: `error!` only for things that abort an operation, `warn!` for
  degraded-but-continuing (an MCP server that failed discovery, an approval that timed out), `info!`
  for lifecycle transitions, `debug!` for per-event detail.

## Done when

- One logging frontend is used across all crates.
- Session/channel/agent ids can be followed through a single request without reading code.
- `AGENTS.md` states the convention (it currently prescribes `log`).

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-22** — Implemented. Decisions:
  - `tracing` everywhere: `log` removed from `caelumora-model`, `caelumora-runtime` and
    `caelumora-tui` (and `env_logger` from the TUI); the unused `tracing-subscriber` dependency
    removed from the three client crates, since libraries must not choose a subscriber. Records from
    dependencies still using `log` are bridged by `tracing-subscriber`.
  - One setup for all binaries, `caelumora_runtime::logging::init`: `RUST_LOG` filters;
    `AppConfig.logging` picks `text` (default) or `json` and an optional file. REST server logs to
    stderr, TUI to `local/tui.log`.
  - Correlation through the existing `EventContext` ids rather than `tracing` span parenting (which
    does not cross channels) and without OpenTelemetry. Spans carry them under their own names:
    `span_id`, `event_id`, `correlation_id`, `depth`. An intermediate version named the log field
    `trace` and, briefly, reused the message id as `span_id`; both were dropped as misleading - a
    field keeps the meaning it has in `EventContext`.
  - To get the `span_id` back to the REST handler, `UserActor` now works with envelopes on both of
    its internal channels: `send_message` / `send_approval` build the envelopes (so their sender and
    security context are always the actor's own) and return the `span_id`; the two envelopes of one
    message share it. The outgoing side became an optional `subscribe()` with `try_send`: before,
    `actor_tx.send().await` on a queue of 100 that the REST server never read would, after 100
    routed events, have stalled the actor's loop and with it everything the user sent.
  - Spans: `request` (method, uri, `user_id`, `span_id`), `envelope` (in `ChannelLoop` and for
    events received by an idle agent), `turn` / `agent` for each iteration of the agent loop.
  - Levels: per-message and per-store-operation logs moved from `info` to `debug` (JSON store
    reads/writes, "Starting generating messages", `wakeup()`, tool execution); streamed chunks to
    `trace`; "Too many iterations" to `warn`. Full envelopes (with content) only at `trace`.
  - Verified against the REST server: one `span_id` from the request span finds 25 lines from REST
    handler through channel, moderator and the agent's turn to its answer; no message content in the
    `debug` log.

