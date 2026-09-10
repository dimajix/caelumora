# 02 — Better error messages

**Status:** open
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
