# Coding Guidelines

The general rules for code in Caelumora. What a rule is for is described in the documents on the respective subject; see [ARCHITECTURE.md](ARCHITECTURE.md).

## Coding Style

### Comments and Documentation
*   **Doc Comments**: Use `///` for public APIs (structs, traits, methods). Document parameters, return values, and potential errors.
*   **Internal Comments**: Use `//` for implementation details. Explain *why* something is done if it's not obvious from the code.
*   **Module Docs**: Use `//!` at the top of new modules to describe their purpose.
*   **Consistency**: Mirror the existing comment frequency and style in the module you are editing.

### Error Handling
*   Use `anyhow::Result` for general application logic, especially in the `runtime` where diverse error types are aggregated.
*   Use `thiserror` for defining domain-specific error types (e.g., in `caelumora-model`).
*   Avoid `unwrap()` or `expect()` in production code. Use `?` or handle errors explicitly.
*   **Messages name what failed.** An error caused by configuration names the component id (and kind) and the parameter; one caused by a remote system names the endpoint (without credentials — see `LLMError::redact_endpoint`). They never contain secret values or file paths, because `caelumora_runtime::error::user_message` shows domain errors (`LLMError`, `ComponentError`, `ToolError`, `TemplateError`, `StoreError` except `Backend`) to users. A new error type meant for users must meet this and be added there; anything else stays in the log.
*   **Errors in a turn end the turn, not the agent.** `AgentActor` reports them as `ActorEvent::TurnFailed` and returns to `Idle`. Don't let a turn-time error escape `AgentActor::run`.
*   **REST handlers** return `Result<_, crate::error::ApiError>`, which answers with RFC 9457 problem details (`application/problem+json`, with a stable `code` and the `request_id`). Use `ApiError::internal` for server errors — it logs the full chain and discloses only what `user_message` allows — and the `Json`/`Path` extractors from `crate::error`, so that malformed requests get problem details too. Never map an error to a bare status code.

### Asynchronous Programming
*   Caelumora makes heavy use of `async/await`.
*   Prefer `tokio` for async primitives (e.g., `Mutex`, `RwLock`, `mpsc`).
*   Traits with async methods should use `#[async_trait]` if they need to be object-safe or if you are working in a part of the codebase already using it. Note: Many internal traits use `BoxStream` for streaming responses.

### Logging
*   **Frontend**: `tracing` in every crate. Libraries never install a subscriber; binaries call `caelumora_runtime::logging::init`.
*   **Levels**:
    *   `error!` only for failures that abort an operation.
    *   `warn!` for degraded-but-continuing (an MCP server that failed discovery, a queue that dropped an event).
    *   `info!` for lifecycle transitions (a channel or actor starting or stopping, a session created).
    *   `debug!` for per-event detail. Anything that happens once per message or per store operation is `debug!` at most; once per streamed chunk is `trace!`.
*   **Content**: message text, prompts and tool arguments only at `trace!`. At `debug!` and above, log ids, event kinds (`ActorEvent::kind`, `Envelope::describe`) and sizes. Credentials, tokens and cookies are never logged at any level.
*   **Correlation**: ids belong on spans, not repeated in messages. Work on an envelope runs in `logging::envelope_span`; its fields are named after the `EventContext` fields (`span_id`, `event_id`, `correlation_id`, `span_depth`), so one `span_id` finds everything a message caused.

## Testing Strategy

### Unit Tests
*   **Placement**: Put unit tests in a `mod tests` block at the bottom of the file, guarded by `#[cfg(test)]`.
*   **Async Tests**: Use `#[tokio::test]` for tests involving async code.
*   **Naming**: Use descriptive names for test functions (e.g., `state_history_appender_is_passthrough_and_persists_history`).
*   **Assertions**: Use `assert!`, `assert_eq!`, and `expect()` with descriptive messages.

### Integration Tests
*   Place integration tests in the `tests/` directory of the relevant crate or in the `examples/` directory if they serve as documentation.


## Checklist

*   When adding new components, remember to register them in the appropriate `ComponentRegistry`.
*   If you modify `caelumora-model`, ensure compatibility with `caelumora-runtime`.
*   Log through `tracing` (`tracing::{debug, info, warn, error, trace}`), never through `log` or `println!`. See *Logging* above.
*   When changing the REST API in `caelumora-rest-server`, ensure compatibilizy with `caelumora-web-ui`

## Logging

All crates log through `tracing`; records from dependencies that still use `log` are forwarded into it. Binaries install the subscriber with `caelumora_runtime::logging::init`: `RUST_LOG` selects the records, `AppConfig`'s `logging` section the format (`text`, the default, or `json`) and an optional file. The REST server logs to stderr, the TUI to `local/tui.log` so that log lines do not break its prompt.

A message crosses several tasks — REST handler, `UserActor`, `ChannelLoop`, moderator, `AgentActor` — connected by channels, so `tracing`'s span nesting alone cannot follow it. The ids of the `EventContext` do: every span that works on an envelope carries them as fields named like the `EventContext` fields (`span_id`, `event_id`, `correlation_id`, `depth`). There are three kinds of span:

*   `request` — one REST request, with `method`, `uri`, the authenticated `user_id`, and the `span_id` of the envelopes the request started (`UserActor::send_message` builds the envelopes and returns it).
*   `envelope` — handling one envelope in the `ChannelLoop` or an idle agent, with session, channel, sender and event kind.
*   `turn` — one iteration of an agent's cognitive cycle, with agent and state and the ids of the turn's `EventContext`; an idle or sleeping agent runs in an `agent` span instead.

Searching the log for one `span_id` therefore finds the request, the routing and the agent's turn that answered it. Message content appears only at `trace` level.

## Error Handling

Errors carry what failed, and only what may be shown goes to users (issue 02).

*   **Domain errors name their subject.** `ComponentError::Build` wraps every failure of a component graph with the component's id and kind; `LLMError` names the endpoint (credentials stripped), the model and the backend's own explanation (`Connection`, `Rejected`, `Backend`, `InvalidResponse`).
*   **What users may see** is decided in one place, `caelumora_runtime::error::user_message`: it looks through an error chain for a domain error written to be read (`LLMError`, `ComponentError`, `ToolError`, `TemplateError`, `StoreError` other than `Backend`) and returns its message; anything else stays in the log.
*   **A failing turn ends the turn, not the agent.** If an error occurs while an `AgentActor` is in a turn — an unreachable model, a fail-closed component, a failing tool executor — the agent logs it, discards the turn's unfinished work and emits `ActorEvent::TurnFailed { message }`, then returns to `Idle`. Moderators forward it to the users; it is broadcast to the UI but never written to the history, so it cannot end up in a model's context ([15](../issues/15-disallowed-tool-errors-to-llm.md) is about errors the model *should* see).
*   **REST errors** are RFC 9457 problem details (`application/problem+json`) with two extension members: `code`, a stable identifier (also the last segment of `type`, `urn:caelumora:problem:<code>`), and `request_id`, which also appears as the `x-request-id` header and on the request's log span. Client errors state their cause; server errors state it only if `user_message` allows, and otherwise point to the log. `ensure_session` distinguishes a session that does not exist (`404`) from one that cannot be started (`500`, naming e.g. the broken component).
