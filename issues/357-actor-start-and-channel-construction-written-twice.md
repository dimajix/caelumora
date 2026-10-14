# 357 — Starting an actor and building a channel's loop are sequences written twice

**Status:** done
**Area:** Code structure

## The problem

Two construction sequences exist in more than one copy:

- **Starting a member's actor.** `ChannelLoop::new` (`channel/mod.rs:319`), `take_in`
  (`channel/participants.rs:92`) and `rejoin` (`:202`) each do `create_actor`, `catch_up` (the two
  later ones), `adopt`, `ActorChange::Start`; `take_in` and `rejoin` both wait with `left` first.
  `rejoin` is `take_in` without storing the document; its doc says so.
- **Building a channel's loop.** `SessionLoop::new` (`session/mod.rs:231`) builds a
  `ChannelRuntime`, a `ChannelLoop`, inserts a `ChannelEntry` and queues the start, then attaches
  every channel in a second loop; `add_channel_loop` (`session/channels.rs:183`) does the same for
  one channel, with the attach inline.
  `ChannelRuntime::new(Arc::clone(&runtime_manager), Arc::clone(&commands), Arc::clone(&session_controller))`
  is written three times (`session/mod.rs:214`, `:232`, `session/channels.rs:184`).

`SessionLoop::new` cannot call `add_channel_loop` because that needs `self: &Arc<Self>` for
`attach`, which [123](123-session-handle-in-channel-runtime.md) chose on purpose.

## Why it matters

What an actor needs before it runs - its predecessor gone, the history it may see taken in, the
moderator and the mentions told - is a rule of the channel, and it is kept by three copies. The
next step in it (a check, a log line) goes into one of them first.

## Considerations

- `ChannelLoop::start_member(&self, member: Member, catch_up: bool) -> Result<ChannelActor>` doing
  `left`, `create_actor`, `catch_up`, `adopt`, `ActorChange::Start`; `rejoin` goes, `take_in`
  stores the document and calls it, undoing the store if it fails as today; `new` calls it per
  participant without the catch-up and with the "cannot run, left out" handling around it.
- `SessionLoop::new` builds the `Arc` with an empty channel table and then calls
  `add_channel_loop` for every channel the session lists; `add_channel_loop` is the one place that
  builds a loop, and `ChannelRuntime` is built by one `fn channel_runtime(&self) -> ChannelRuntime`
  that `views_of` uses too. The order of events (`STARTED` after `created`) does not change, since
  `run` starts the channels either way. This keeps `attach` as [123](123-session-handle-in-channel-runtime.md)
  decided and does not reopen it.

## Done when

- One method starts a member's actor, one builds a channel's loop, one builds a `ChannelRuntime`.
- No behaviour changes; the tests of joining, rejoining (`follow_agents`) and creating channels pass
  unchanged.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
- **2026-10-14** — Its second half is done with [361](361-session-before-its-channels.md):
  `SessionLoop::new` builds its channels through `add_channel_loop`, and
  `SessionLoop::channel_runtime` is the one place a `ChannelRuntime` is made for a session. Starting
  a member's actor is what is left.
- **2026-10-14** — Done. `ChannelLoop::start_member(&self, member)` waits for `left`, builds the
  actor, catches it up, adopts it and hands it to `run`; `rejoin` is gone, `follow_agents` calls
  it, and `take_in` calls it after storing, undoing the store if it fails as before. `take_in`
  still waits for `left` before storing, so that nothing is stored for a participant whose old
  actor hangs; the wait in `start_member` then finds nothing left. `ChannelLoop::new` does not go
  through it, unlike the consideration above: an actor of a channel being built is not caught up
  and not handed to `run` - `run` starts every actor it finds when it begins, and a queued
  `ActorChange::Start` could start an actor again whose task had already ended. It builds and
  adopts alone. `create_actor` is a method taking only the member, no longer the controller and
  the runtime it can find on `self`.
