# Agent Guidelines

Welcome, coding agent! This document provides guidelines for contributing to Caelumora. It covers coding style, testing, and other relevant topics to ensure consistency and quality across the codebase.

## Architectural Overview
Before making any changes, please refer to [ARCHITECTURE.md](ARCHITECTURE.md) for a detailed explanation of the project's design, including:
*   Top-level module structure
*   Component architecture
*   The `AgentLoop` cognitive cycle
*   The Actor-based communication model

## Issue Tracking

Open issues live in this repository, not in an external tracker. The overview
is [issues/INDEX.md](issues/INDEX.md); every issue additionally has its own
file `issues/<number>-<slug>.md` holding the reasoning, the options that were
weighed, and the decision.

Read `issues/INDEX.md` before starting work. If what you are about to do
corresponds to an issue, work from that issue's file — it usually records why
an obvious approach was already rejected.

### Keeping issues up to date

*   **An issue is two places.** A line in `issues/INDEX.md` *and* a file
    `issues/<number>-<slug>.md`. Create both together; never delete either.
*   **Status lives in both places** and must be changed in both when it moves.
    The valid values are `open`, `in progress`, `done` and `rejected`.
    `rejected` means: considered and deliberately not done, with the reason
    recorded — that reason is the whole point, otherwise it gets re-derived
    six months later.
*   **Never rewrite the description of an issue that has been resolved.** The
    original text is the record of the problem, not of the solution. Append
    what happened under `## History` at the end of the issue file, dated, as a
    new bullet. Outdated text stays.
*   **Numbers are permanent.** They are never reused and never reassigned —
    commits, code comments and other issues refer to them. A new issue takes
    the next free number even if lower ones are closed. Write them with two
    digits (`01`, not `1`) so the directory sorts correctly; from `100`
    onwards, three.
*   **The index line is one sentence.** It says what is wrong, not how it will
    be fixed. Everything longer belongs in the issue file, where length is not
    a problem.

### Creating a new issue

1.  Check `issues/INDEX.md` first — an issue for it may already exist, in
    which case add to that file's `## History` rather than opening a second
    one.
2.  Take the next free number and create `issues/<number>-<slug>.md` with the
    status header, the problem, why it matters, the considerations (options
    and their trade-offs), a "done when", and a first `## History` entry.
3.  Add one line to the matching section of `issues/INDEX.md`. If no section
    fits, add one.
4.  Cross-reference related issues by number, e.g. `[19](19-memory-tiers.md)`,
    where one genuinely blocks or overlaps another.

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

## Contribution Workflow
1.  **Analyze**: Understand the requirement and how it fits into the architecture. Check [issues/INDEX.md](issues/INDEX.md) for an existing issue covering it.
2.  **Plan**: Identify the files that need modification and the tests required.
3.  **Implement**: Apply minimal, focused changes following the coding style.
4.  **Document**: Update `ARCHITECTURE.md` and/or `README.md` if your changes introduce new architectural concepts, modules, or significant features.
5.  **Verify**: Run existing tests and add new ones to cover your changes.
6.  **Refactor**: Ensure the code is clean and well-documented.
7.  **Record**: If the change resolves or advances an issue, update its status in *both* `issues/INDEX.md` and the issue file, and append a dated entry under `## History`. If it uncovered something new, open an issue for it rather than leaving it in a comment.

### Architecture decisions
Architecture questions are discussed before they are implemented: where a
method lives, which object owns a responsibility, how context reaches the
code that needs it, new types or layers. Lay out the options with their
trade-offs, give a recommendation, and wait for the decision. Proposals from
the maintainer are to be assessed honestly, not just accepted.

Prefer the clean structural solution over a pragmatic shortcut, even when it
touches more code — and say what it costs. Glue code that feels awkward (one
object making another act on its behalf, a parameter that only works around
missing ownership) is a sign that something sits on the wrong object; raise
it rather than building around it.

Solve problems along the patterns the codebase already has before inventing
new ones: a new capability that resembles an existing one is built the same
way (commands like tools: a `…Provider` component facet, an `…Executor` that
receives ready providers and checks permissions). When a proposal adds a
second implementation of something that exists, look for differences between
the two that have no reason — naming, construction, error handling — and
propose aligning them, in whichever direction is the better one. A deliberate
difference is fine, but say why it exists.

A pure access question - "may they?" asked from outside an entity, to
decide what to list or offer - is a method of
`caelumora_model::security::AccessQuestions`, not of the entity (issue 71).
An operation that checks its permission as part of what it does keeps that
check itself, and makes it with `Enforcement::require` (issue 95), which
answers a refusal with the operation's own error and logs it.

### Branches
Until further notice, work and commit directly on `main`. Do not create
feature branches unless asked to.

## Agent-Specific Tips
*   When adding new components, remember to register them in the appropriate `ComponentRegistry`.
*   If you modify `caelumora-model`, ensure compatibility with `caelumora-runtime`.
*   Log through `tracing` (`tracing::{debug, info, warn, error, trace}`), never through `log` or `println!`. See *Logging* below.
*   When changing the REST API in `caelumora-rest-server`, ensure compatibilizy with `caelumora-web-ui`
