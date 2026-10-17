# 361 — A channel is attached to its session in a second step, though the session's controller already travels through its runtime

**Status:** done
**Area:** Code structure
**Epic:** [349](349-core-review-epic.md)
**Summary:** A channel is attached to its session in a second step after it is built, though the session's controller already travels through its runtime; the option 123 rejected, on code that has changed since.

## The problem

A `ChannelLoop` reaches its session through `session: OnceLock<Weak<SessionLoop>>`, set by
`ChannelLoop::attach` after the loop exists (`channel/mod.rs:247`, `:361`), because
`SessionLoop::new` builds its channels before the `Arc<SessionLoop>` exists and attaches them in a
second loop (`session/mod.rs:231-258`); `add_channel_loop` attaches inline
(`session/channels.rs:186`). [123](123-session-handle-in-channel-runtime.md) chose this on
2026-09-29 over building the session first and handing it to `ChannelRuntime::new`, because that
"needs `SessionLoop::new` in two phases and a test-only constructor for channels without a
session; the session would still travel through the runtimes".

Both reasons have changed since:

- `ChannelRuntime` carries the session's controller already (`session: Arc<SessionController>`,
  `runtime.rs:96`, since issue 13), read by `ActorRuntime::session_controller` for the tool
  executor; so a session does travel through every actor's runtime, as a controller rather than a
  loop.
- The test-only constructor exists: `ChannelRuntime::outside_a_session` (`runtime.rs:299`), which
  makes a bare session controller for a channel of no running session, and
  `SessionLoop::views_of` builds a `ChannelRuntime` for tests as well (`session/mod.rs:213`).
- `SessionLoop::new` is in two phases anyway: build the loops, make the `Arc`, attach.

So the channel reaches its session in two ways - the controller through its runtime, the loop
through `attach` - and a channel exists for a moment without a session, which `session_loop()`
answers with `None` and `workspace_terminal`, `take_in` and the views handle as an error case
(`NotUsableInChannel`, `NotInSession`).

## Why it matters

Two handles to one session, set at two times, are two things to keep consistent, and a state - the
channel between `new` and `attach` - that every reader of the session has to allow for although it
never lasts. [357](357-actor-start-and-channel-construction-written-twice.md) wants
`SessionLoop::new` to build its channels through `add_channel_loop`, which needs the `Arc` first;
with the session first, the attach step is not needed at all.

## Considerations

- **The session first, the channel given it.** `SessionLoop::new` makes the `Arc` with an empty
  channel table, then `add_channel_loop` per channel; `ChannelRuntime::new` takes the
  `Weak<SessionLoop>` and reads the session's controller through it when asked, or keeps the
  controller beside it. `ChannelLoop::attach`, its `OnceLock` and the "not attached yet" case go;
  `session_loop()` answers `None` only once the session is gone. `outside_a_session` stays the
  test's way to a channel without a session, with the bare controller it makes today.
- **The controller through the loop.** With the loop in the runtime,
  `ActorRuntime::session_controller` can upgrade it and ask the loop; `ChannelRuntime::session`
  goes. If a stopped session must still
  serve a running actor's tool guard - it does not: the session's channels stop with it -, the
  controller stays beside the loop.
- **Not again:** the session handle on `ActorRuntime` for actors to read; [123](123-session-handle-in-channel-runtime.md)
  removed it and the views read the session through the channel since. The runtime carries the
  handle for the channel, not for the actors.

An architecture decision, to be discussed with the maintainer before
[357](357-actor-start-and-channel-construction-written-twice.md), which it decides the shape of.

## Done when

- A channel is given its session when it is built, and no `attach` step exists outside
  `SessionLoop`.
- The channel reaches its session's loop and controller one way.
- No behaviour changes; the tests of sessions, channels and the workspace's use in a channel pass.

## History

- **2026-10-14** — Raised again by the maintainer after the review of agents, sessions and
  channels (epic [349](349-core-review-epic.md)): the option [123](123-session-handle-in-channel-runtime.md)
  rejected, on code that has changed since.
- **2026-10-14** — Decided with the maintainer: the session first, the channel given it through its
  runtime; the controller stays beside the loop, since the guard of the tool calls must not depend
  on a `Weak` and the tests outside a session have a controller only. Not taken: the loop as a
  parameter of `ChannelLoop::new` beside the runtime, which kept two ways to the session; the
  controller through the loop alone, which would have made the guard optional.
- **2026-10-14** — Done. `ChannelRuntime` holds the session's `Weak<SessionLoop>` beside its
  controller, read by `ChannelLoop::session_loop` and not passed on by `ActorRuntime`; the
  channel's `OnceLock` and `ChannelLoop::attach` are gone. `SessionLoop::channel_runtime` is the one
  place a `ChannelRuntime` is made for a session, and `SessionLoop::new` makes the `Arc` first and
  builds its channels through `add_channel_loop` - which takes the second half of
  [357](357-actor-start-and-channel-construction-written-twice.md) with it. `ChannelRuntime::new`
  stays public: the MCP plugin's integration test builds a runtime outside a session, with a
  dangling `Weak`. The `ChannelHandle` by which actors reach their channel keeps its attach step:
  the actors are built before the channel's `Arc` exists.
