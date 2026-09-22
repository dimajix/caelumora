# 02 — Better error messages

**Status:** done
**Area:** Developer & user experience

## The problem

Errors reaching a user or a caller are frequently stripped of the context that
would make them actionable. The runtime aggregates diverse failures into
`anyhow::Result`, which is the right choice for the runtime itself, but the
context is often not attached on the way out: a failing tool call, an
unreachable LLM backend and a malformed component configuration can all
surface as a bare string with no indication of which agent, which component or
which session produced it.

At the REST boundary the loss is worse — several handlers translate a failure
into a status code and discard the cause entirely.

## Why it matters

A misconfigured component or an unreachable model backend is the most common
kind of failure in day-to-day use, and it is precisely the kind that a good
message resolves in seconds and a bad one turns into a debugging session.

## Considerations

- `thiserror` domain errors in `caelumora-model` (as `AGENTS.md` already
  prescribes) give callers something to match on; `anyhow` context in the
  runtime gives humans something to read. Both are needed, at different
  layers.
- Errors that are caused by configuration should name the offending component
  id and parameter. Errors caused by a remote system should name the endpoint.
- The REST layer needs a decision on how much to disclose: a structured error
  body is far more useful than a status code, but it must not leak secret
  values or internal paths. See also [10](10-user-authentication.md).
- Related but distinct: an error that occurs while the agent is running should
  usually also reach the *model*, not just the log — see
  [15](15-disallowed-tool-errors-to-llm.md).

## Done when

- Configuration and connectivity failures name the component and the cause.
- REST responses carry a structured error body instead of a bare status code.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-22** — Implemented. Decisions and findings:
  - **Turn failures.** An error during a turn used to escape
    `AgentActor::run`, which ended the agent's task for the rest of the
    session, logged as "Failed to spawn actor", with the UI stuck on
    "generating". Now the agent logs the error, discards the unfinished turn,
    emits the new `ActorEvent::TurnFailed { message }` and goes back to
    `Idle`. Decided: the event is shown to users (moderators forward it to
    user actors, the channel broadcasts it) but not written to the history,
    so it cannot reach a model's context - that remains
    [15](15-disallowed-tool-errors-to-llm.md). Consequence: after a reload
    the failure is no longer shown.
  - **What users see** is decided in `caelumora_runtime::error::user_message`:
    the first domain error in the chain that is written to be read
    (`LLMError`, `ComponentError`, `ToolError`, `TemplateError`, `StoreError`
    except `Backend`, which may carry paths); otherwise a generic message
    pointing to the log.
  - **Configuration and connectivity.** `ComponentError::Build` names
    component id and kind for every failure of a component graph.
    `LLMError` now has `Connection`, `Rejected`, `Backend` and
    `InvalidResponse`, each with the endpoint (credentials stripped); the
    Ollama client also reports the model and the backend's own message (e.g.
    "model 'x' not found", previously lost behind "404 for url"), reports
    `{"error": ...}` lines in a stream (previously ignored), reports a stream
    that ends without a final message, and no longer logs model output at
    `warn`.
  - **REST.** RFC 9457 problem details (`application/problem+json`) with
    extension members `code` and `request_id`; `type` is
    `urn:caelumora:problem:<code>`. Client errors state their cause, server
    errors only what `user_message` allows. Decided in addition: every
    request gets a `request_id` (log span field, `x-request-id` header, error
    body), because the `span_id` from [01](01-improve-logging.md) exists only
    for requests that send into a channel, not for e.g. a session that fails
    to load. `ensure_session` now tells "does not exist" (404) from "cannot
    be started" (500 naming the cause) - before, every load failure was a
    404. `Json`/`Path` extractors in `crate::error` turn malformed requests
    into problem details as well. Login failures do not disclose a
    provider's own error text.
  - **Panics removed**: listing templates (two handlers used `.expect`), a
    tool call with a non-agent caller, a missing personality or failing agent
    creation while instantiating a template, a failing store write when
    renaming an agent or a session. The remaining `unwrap`s in production
    code guard invariants.
  - **UI.** Web UI: errors of any API call appear as a dismissible banner
    with the server's detail and the `request_id`; a failed turn appears in
    the chat with the agent's name and the `span_id`, and ends the
    "generating" state; a message that could not be sent is put back into
    the input. TUI prints failed turns.
  - Verified against the REST server: with an unreachable backend (URL with
    credentials) the agent reports "cannot reach LLM backend at
    http://127.0.0.1:1: ... Connection refused" twice in a row - it survives
    the first failure - and the credentials appear neither in the message nor
    in the log; with an unknown model it reports "rejected the request for
    model 'does-not-exist:1b' (HTTP 404): model 'does-not-exist:1b' not
    found"; missing login, missing session, invalid UUID, malformed JSON,
    wrong content type and unknown template all answer with problem details.

