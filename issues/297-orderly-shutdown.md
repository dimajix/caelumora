# 297 — Caelumora ends without an orderly shutdown

**Status:** done
**Area:** Observability and operations

## The problem

The REST server ends with `axum::serve(listener, app).await?`: no signal is caught, nothing is
stopped. SIGINT or SIGTERM end the process where it stands; so does leaving the TUI, which builds
the runtime itself (`caelumora-tui/src/main.rs`). Then

*   the agents' turns end mid-step, and no actor is given the time to store its state on its way
    out, as `stop_session` gives it (`stop_actors`, issue 187);
*   a stream of server-sent events breaks off instead of being closed;
*   a command of the `local` shell is left running, its process group known to nothing after a
    restart (issue [254](254-commands-run-unbounded.md)) - an Open Terminal instance keeps its
    commands, which end at the storage's `max_run_secs`.

## Why it matters

A deploy or a restart is an ordinary event, not a failure, and should leave what was running as it
would be left by stopping its session: state stored, commands ended, clients told. Today only a
session stopped by hand ends that way.

## Considerations

- **What it takes.** A signal handler (`tokio::signal`, SIGINT and SIGTERM) and
  `serve(...).with_graceful_shutdown(...)`; a way to end the streams, since a graceful shutdown
  waits for every connection and the global stream never ends by itself (`routes/events.rs` ends on
  `std::future::pending()`) - e.g. a `CancellationToken` in `AppState`; then a
  `SessionManager::shutdown()` that stops every running session as `stop_session` does - which also
  ends their commands (254) - and ends what a storage still lists. The same for the TUI.
- **A bound.** Everything within the time a supervisor leaves between SIGTERM and SIGKILL (Docker
  10 s, systemd 90 s): commands ended in parallel, actors given a bounded time, the rest dropped. A
  shutdown that hangs is worse than none.
- **Open Terminal's commands run on.** After a restart the instance still knows a command, and the
  model can follow it by its id; ending it at shutdown would take that away, and its bound ends it
  anyway. Only the `local` shell's commands are ended.
- **What it does not close.** SIGKILL, the OOM killer, a crash or a power loss pass any shutdown:
  orphans of the `local` shell stay possible. Closing that would take remembering its process
  groups beyond the process (a file under the storage's root) and killing them at start - with the
  risk of a process id used again. `local` is for development; whether that is worth it is open.

- **Resuming after a restart.** Until [386](386-actor-state-after-a-stop.md), an open browser
  started every session it followed when it reconnected to a restarted server, and with it the
  turns stopped at the shutdown. Now a session runs again only when someone acts in it. Whether the
  server should start what ran when it went down, so that interrupted turns go on by themselves, is
  to be decided here.

## Done when

- SIGINT, SIGTERM and leaving the TUI stop the running sessions as `stop_session` does, close the
  streams and end the `local` shell's commands, within a bound.

## History

- **2026-10-10** — Opened while deciding the open points of [254](254-commands-run-unbounded.md)
  with the maintainer: a `local` command left running when Caelumora ends belongs to an orderly
  shutdown, which is worth having for the actors and the streams in the first place.
- **2026-10-16** — Since [386](386-actor-state-after-a-stop.md), following a channel does not start
  its session; whether the server resumes what ran at a shutdown is added to the considerations.
- **2026-10-16** — Decided with the maintainer and done.
  - `SessionManager::shutdown` refuses every start from then on (`SessionError::ShuttingDown`,
    `503 shutting_down`), stops every running session as `stop` does, all at once, and has each
    storage end what does not outlive the process (`TerminalProvider::shutdown`: the `local` shell
    ends its commands, Open Terminal keeps them) - within `SHUTDOWN_BOUND`, a constant of eight
    seconds. Ending the commands per channel moved from `stop_running` to `stop` and `delete`.
  - The server catches SIGINT and SIGTERM, shuts the sessions down, then ends every stream through
    `AppState::shutdown` and waits a second for the requests under way. A channel's stream ends
    after what the user's actor showed before, so its client keeps the channel's state as it
    stopped. Clients are not told why; `EventSource` retries until the server is back.
  - The TUI already stopped its session at `exit`, `quit` and Ctrl-C - the description overstated
    that; it now ends the same way on SIGINT, SIGTERM and SIGHUP, and shuts down instead of
    stopping, so that its Open Terminal commands run on as the server's do.
  - Tried against the running server - SIGTERM while an agent planned: done in 0.2 s, the agent
    stopped, not given up, the channel's stream ended after the stop's snapshot - and the TUI under
    a pseudo-terminal with SIGTERM and SIGHUP.
  - Resuming what a shutdown interrupted is [388](388-resume-interrupted-work-after-a-restart.md).
