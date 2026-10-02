# 186 — Long A2A tasks without streaming, and streamed requests without a timeout

**Status:** open
**Area:** Integrations

## The problem

Reported by a cleanup review of the issue-179 series; **not yet verified** against the A2A 1.0
specification and a real server:

- **A blocking `SendMessage` is bounded by `timeout_secs`** (default 30), though the setting says it
  is "not how long a streamed task may run". On an agent without streaming, a task that takes longer
  fails as a connection error before its id is known, so it is never recorded as open and runs on
  remotely with nobody following it.
- **`SendMessage` is sent without `blocking`.** If the remote side does not block by default, it
  answers at once with a working task; the answer ends there, `GetTask` is asked once per run of the
  actor (the `attempted` set), and the result is fetched only after a restart - contrary to the
  module documentation ("waits for the task to end").
- **Streamed requests have no timeout until the headers arrive** - only `connect_timeout`. A server
  that accepts the connection and never answers keeps `send` waiting forever; in
  `follow_open_tasks` and before the cancellation `select!` of `exchange`, neither cancel nor
  stop reaches it.

## Why it matters

An external agent without streaming does not deliver long tasks, and one that hangs blocks its
actor.

## Considerations

- Send without blocking (or with `returnImmediately`, if A2A 1.0 names it so) and poll `GetTask`
  with a backoff while the task works - which also makes a long task survive a restart.
- Or exempt the blocking `SendMessage` from the per-request timeout - simpler, but keeps a request
  open for as long as the task runs.
- A timeout for the headers of a streamed request, the body unbounded.

## Done when

- A task longer than `timeout_secs` on an agent without streaming is delivered in the same run, and
  a streamed request whose server never answers ends after a bounded time.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series; to be verified first.
