# 360 — `ChannelRuntime` forwards ten services it never uses itself, each read once by `ChannelLoop::new`

**Status:** done
**Area:** Code structure
**Epic:** [349](349-core-review-epic.md)
**Summary:** `ChannelRuntime` forwards ten services of the `RuntimeManager` it never uses itself, each read once by `ChannelLoop::new`; what 89 rejected, on code that has changed since.

## The problem

`ChannelRuntime` (`runtime.rs:91`) has ten methods that pass straight through to
`RuntimeManager`: `security_manager`, `confidentiality`, `secret_manager`, `services`,
`component_registry`, `state_store`, `event_bus`, `controller_manager`, `entity_stores`,
`new_recorder` (`runtime.rs:136-156`). `ActorRuntime` repeats six of them (`runtime.rs:185-195`).
Since [229](229-services-fetched-through-the-runtime.md), `ChannelLoop` holds the services it
works with as fields, so every forwarder of `ChannelRuntime` has exactly one caller left, in
`ChannelLoop::new` (`channel/mod.rs:259-298`), where it is read once into a field. What
`ChannelRuntime` is for has changed meanwhile, too: it carries the session's controller
(`session`, issue 13) and the channel's handle, and is "the only source of `ActorRuntime`s"
(CHANNELS.md) - the actors' runtimes are what it is about, not the channel's services.

[89](89-channel-runtime-delegators.md) rejected replacing six such forwarders by one `manager()`
in 2026-09-29, when they had six callers spread over `channel.rs` and the gain was six lines. Both
sides of that weighing have moved: there are ten forwarders now, and one caller.

## Why it matters

A reader of `ChannelRuntime` sees a facade over the runtime; what it is - what every actor of a
channel is built with - is three fields among thirteen accessors. The six repeated on
`ActorRuntime` are the narrow set an actor may reach ([69](69-scoped-actor-runtime.md)) and stay;
the ten on `ChannelRuntime` guard nothing, since `ChannelLoop` is the crate's own.

## Considerations

- **`ChannelLoop::new` takes the services it keeps, and `ChannelRuntime` keeps what the actors
  need.** `ChannelLoop::new(channel, runtime, services)` with the services as what [356](356-member-questions-threaded-with-their-services.md)
  makes of them, or read from the `RuntimeManager` the session holds, which builds the loop
  anyway (`SessionLoop::add_channel_loop`, [357](357-actor-start-and-channel-construction-written-twice.md));
  `ChannelRuntime` loses the ten forwarders and keeps `new_actor_runtime`, `attach`,
  `session_controller`. The `RuntimeManager` does not travel into the channel: the session reads
  it, as it does today for `new_command_executor`.
- **`ChannelRuntime` offers the `RuntimeManager` to the channel alone** (`pub(super)`-like, or a
  method the channel module alone calls): the one reading in `new` goes through it. This is the
  option 89 rejected; it is cheaper now, with one caller, but it hands out the manager that
  [191](191-channel-runtime-hands-out-runtime-manager.md) took away.
- **Keep them** and record why: if the review of what a channel is built with
  ([357](357-actor-start-and-channel-construction-written-twice.md)) ends with `ChannelRuntime` as
  the one argument of `ChannelLoop::new`, the forwarders are its interface.

To be discussed with the maintainer before [357](357-actor-start-and-channel-construction-written-twice.md)
settles what builds a channel's loop.

## Done when

- `ChannelRuntime` has no method that only forwards to `RuntimeManager`, or this issue records why
  they stay this time.
- No behaviour changes.

## History

- **2026-10-14** — Raised again by the maintainer after the review of agents, sessions and
  channels (epic [349](349-core-review-epic.md)): [89](89-channel-runtime-delegators.md) was
  decided on code that has changed since.
- **2026-10-14** — Decided with the maintainer: they stay, trimmed. All of them were read in
  `ChannelLoop::new` alone, and two not at all. `ChannelLoop::new` taking the `RuntimeManager`
  beside the runtime would pass a service next to the object that already holds it, which
  [CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md) rules out; `ChannelRuntime` handing out the
  manager to the channel would undo [191](191-channel-runtime-hands-out-runtime-manager.md) and
  [359](359-actor-runtime-hands-out-the-runtime-manager.md), since `pub(crate)` cannot keep it to
  the channel module. What stays is constructor injection as the guidelines write it: `new` takes
  the services once from the runtime it is given.
- **2026-10-14** — Done. `secret_manager` and `event_bus`, which nothing called, are gone; the eight
  that `ChannelLoop::new` reads are `pub(crate)`, in an `impl` block of their own that says what
  they are for. Four of them serve only the questions on a member; step 2 of
  [356](356-member-questions-threaded-with-their-services.md) may bring the session's `Membership`
  through `SessionLoop::channel_runtime` instead.
