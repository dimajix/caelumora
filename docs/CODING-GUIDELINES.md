# Coding Guidelines

The general rules for code in Caelumora: style, error handling, asynchronous code, logging and
tests. Rules that belong to one subject live with it - where permission checks go in
[SECURITY.md](SECURITY.md), how a component is written in [COMPONENTS.md](COMPONENTS.md).
[ARCHITECTURE.md](ARCHITECTURE.md) gives the overall picture.

## Comments and documentation

*   **Doc comments:** use `///` for public APIs (structs, traits, methods). Document parameters,
    return values and potential errors.
*   **Internal comments:** use `//` for implementation details. Explain *why* something is done if
    it is not obvious from the code.
*   **Module docs:** use `//!` at the top of new modules to describe their purpose.
*   **Consistency:** mirror the comment frequency and style of the module you are editing.

## Crates and interfaces

*   **Model and runtime.** When you change `caelumora-model`, keep `caelumora-runtime` compatible
    with it.
*   **REST and web UI.** When you change the REST API in `caelumora-rest-server`, keep
    `caelumora-web-ui` compatible with it.
*   **Components.** A new component kind is registered in the `ComponentRegistry`; where it goes and
    how it is named is described in [COMPONENTS.md](COMPONENTS.md).

### Runtimes and their entities

A running object - `SessionLoop`, `ChannelLoop`, `ActorRuntime` - and the stored entity it holds
(`Session`, `Channel`) share their work by three rules (issue 144):

*   **The model derives.** A value derived from an entity is computed in `caelumora-model` - a
    method of the entity (`Agent::security_context`) or a constructor of the derived type
    (`SessionPrivacy::of(&Session)`) - never in the runtime, so that the stored document and the
    running object cannot disagree about it.
*   **A runtime answers about its own entity, not about the one above.** A loop may have getters for
    attributes of its own entity that callers need often; they return owned values, so that no
    `arc_swap::Guard` is held across an `await`. Everything else is read from its snapshot. The
    entity above is reached through the parent - `runtime.channel_loop()`,
    `channel.session_loop()` - never through a forwarder such as a `channel_classification()` on the
    actor's runtime. What holds when the parent is missing is the caller's to decide.
*   **One name, one kind of thing.** `channel()` and `session()` return the entity's document as it
    is now; `channel_loop()` and `session_loop()` the running object above; loading any stored
    entity is `load_…` (`ActorRuntime::load_session`).

## Error handling

Errors carry what failed, and only what may be shown goes to users (issue 02).

### Types

*   Use `anyhow::Result` for general application logic, especially in the runtime, where diverse
    error types are aggregated.
*   Use `thiserror` for domain-specific error types, as in `caelumora-model`.
*   Avoid `unwrap()` and `expect()` in production code. Use `?` or handle the error explicitly.

### Messages name what failed

*   An error caused by configuration names the component id (and kind) and the parameter.
    `ComponentError::Build` wraps every failure of a component graph with the component's id and
    kind.
*   An error caused by a remote system names the endpoint, without credentials
    (`remote::redact_endpoint`), and the chain of its causes (`remote::describe_chain`). `LLMError`
    names the endpoint, the model and the backend's own explanation (`Connection`, `Rejected`,
    `Backend`, `InvalidResponse`).
*   Messages never contain secret values or file paths, because they may be shown to users.

### What users see

What users may see is decided in one place, `caelumora_runtime::error::user_message`. It looks
through an error chain for a domain error written to be read - `LLMError`, `ExternalAgentError`,
`ComponentError`, `ToolError`, `TemplateError`, `StoreError` other than `Backend` - and returns its
message. Anything else stays in the log.

A new error type meant for users must meet the rules above and be added to `user_message`.

### Errors in a turn end the turn, not the agent

If an error occurs while an `AgentActor` is in a turn - an unreachable model, a fail-closed
component, a failing tool executor - the agent logs it, discards the turn's unfinished work, emits
`ActorEvent::TurnFailed { message }` and returns to `Idle`. Don't let a turn-time error escape
`AgentActor::run`.

Moderators forward `TurnFailed` to the users. It is broadcast to the UI but never written to the
history, so it cannot end up in a model's context. Errors the model *should* see are the subject of
[15](../issues/15-disallowed-tool-errors-to-llm.md).

### REST errors

REST handlers return `Result<_, crate::error::ApiError>`. Never map an error to a bare status code;
the server answers every error with problem details, as described in
[REST-SERVER.md](REST-SERVER.md#errors).

*   Use `ApiError::internal` for server errors: it logs the full chain and discloses only what
    `user_message` allows.
*   Use the `Json` and `Path` extractors from `crate::error`, so that malformed requests get problem
    details too.

## Asynchronous programming

*   Caelumora makes heavy use of `async`/`await`.
*   Prefer `tokio` for async primitives (`Mutex`, `RwLock`, `mpsc`, ...).
*   Traits with async methods use `#[async_trait]` if they need to be object-safe, or if the
    surrounding code already uses it. Many internal traits use `BoxStream` for streaming responses.

## Logging

### Setup

*   Every crate logs through `tracing` (`tracing::{debug, info, warn, error, trace}`), never through
    `log` or `println!`. Records from dependencies that still use `log` are forwarded into
    `tracing`.
*   Libraries never install a subscriber. Binaries call `caelumora_runtime::logging::init`.
*   `RUST_LOG` selects the records; the configuration's `logging` section sets the format (`text`,
    the default, or `json`) and an optional file (see [CONFIGURATION.md](CONFIGURATION.md)). The
    REST server logs to stderr, the TUI to `local/tui.log`, so that log lines do not break its
    prompt.

### Levels

*   `error!` only for failures that abort an operation.
*   `warn!` for degraded but continuing operation: an MCP server that failed discovery, a queue that
    dropped an event.
*   `info!` for lifecycle transitions: a channel or actor starting or stopping, a session created.
*   `debug!` for per-event detail. Anything that happens once per message or per store operation is
    `debug!` at most; once per streamed chunk is `trace!`.

### Content

*   Message text, prompts and tool arguments only at `trace!`.
*   At `debug!` and above, log ids, event kinds (`ActorEvent::kind`, `Envelope::describe`) and
    sizes.
*   Credentials, tokens and cookies are never logged, at any level.

### Correlation

A message crosses several tasks - REST handler, `UserActor`, `ChannelLoop`, moderator,
`AgentActor` - connected by channels, so `tracing`'s span nesting alone cannot follow it. The ids of
its `EventContext` can. Ids therefore belong on spans, not repeated in messages.

Every span that works on an envelope carries the ids as fields named after the `EventContext`
fields: `span_id`, `event_id`, `correlation_id` and `span_depth`. There are three kinds of span:

*   **`request`** - one REST request, with `method`, `uri`, `request_id`, the authenticated
    `user_id`, and the `span_id` of the envelopes the request started (`UserActor::send_message`
    builds the envelopes and returns it).
*   **`envelope`** - handling one envelope in the `ChannelLoop` or an idle agent
    (`logging::envelope_span`), with session, channel, sender and event kind.
*   **`turn`** - one iteration of an agent's cognitive cycle, with agent and state and the ids of
    the turn's `EventContext`. An idle or sleeping agent runs in an `agent` span instead.

Searching the log for one `span_id` therefore finds the request, the routing and the agent's turn
that answered it.

## Testing

### Unit tests

*   **Placement:** in a `mod tests` block at the bottom of the file, guarded by `#[cfg(test)]`.
*   **Async tests:** use `#[tokio::test]` for tests involving async code.
*   **Naming:** use descriptive names, e.g.
    `an_answer_is_its_whole_messages_and_what_was_being_written`.
*   **Assertions:** use `assert!`, `assert_eq!` and `expect()` with descriptive messages.

### Integration tests

Integration tests go into the `tests/` directory of the relevant crate, or into `examples/` if they
serve as documentation.
