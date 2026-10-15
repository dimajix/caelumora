# 366 — `runtime.rs` holds the runtime, how it is built, and what a channel and an actor are given of it, in one file

**Status:** done
**Area:** Code structure

## The problem

`caelumora-runtime/src/runtime.rs` (847 lines, without a description of the module) holds four
types of their own, interleaved:

| Lines | What |
|---|---|
| 46-93, 653-780 | `RuntimeManager`, the process-wide services and managers - its struct at the top, its methods after the builder |
| 94-183 | `ChannelRuntime`, what a channel is built with ([360](360-channel-runtime-forwarders-again.md)), and `ChannelHandle` |
| 184-340 | `ActorRuntime`, the facade an actor is given ([359](359-actor-runtime-hands-out-the-runtime-manager.md)), and a test constructor of `ChannelRuntime` |
| 341-652 | `RuntimeBuilder`, how a runtime is built from the configuration |
| 781-847 | the tests |

## Why it matters

The struct of `RuntimeManager` and its methods are 560 lines apart; whoever reads what an actor
may reach of the runtime reads past how the runtime is built. Each of the four has a responsibility
of its own and is reviewed and changed on its own - 359, 360 and 361 each touched one of them.

## Considerations

- **A module `runtime/`**, the paths unchanged through re-exports:
  - `mod.rs`: the description of the module - what the runtime is, and what of it a channel and an
    actor are given -, and the re-exports;
  - `manager.rs`: `RuntimeManager`, struct and methods together, by the rule of
    [364](364-managers-in-different-places.md);
  - `builder.rs`: `RuntimeBuilder`;
  - `channel.rs`: `ChannelRuntime` and `ChannelHandle`, with the test constructor;
  - `actor.rs`: `ActorRuntime`.
  Each with its tests.
- **Open:** whether `RuntimeBuilder` goes into `manager.rs`, as what builds the manager. It is the
  larger half (310 lines) and changes for other reasons - the configuration, the plugins -, so a
  file of its own is proposed.
- Moves only, no behaviour changes; `runtime.rs` through `git mv` to the file that keeps most of
  it, so that its history stays readable.

## Done when

- `runtime.rs` is a module `runtime/` with one file per type as above, and every path into it
  stays as it was.
- No behaviour changes; clippy and the tests pass.

## History

- **2026-10-15** — Opened at the maintainer's request, under epic [349](349-core-review-epic.md),
  split off [364](364-managers-in-different-places.md).
- **2026-10-15** — Done, as considered; no behaviour changes.
  - `runtime.rs` went to `runtime/builder.rs` (`git mv`), which keeps the largest part:
    `RuntimeBuilder`, a file of its own as proposed, with the test of the backends the
    configuration names.
  - `manager.rs` holds `RuntimeManager`, its struct and its methods together, with the test of the
    configured defaults; its fields are `pub(super)`, since `RuntimeBuilder::build` makes it.
  - `channel.rs` holds `ChannelRuntime`, `ChannelHandle` and the constructors for tests
    (`outside_a_session`, `restarted`); `actor.rs` holds `ActorRuntime` and its test. What the
    two reach of each other - the fields of `ChannelRuntime` an `ActorRuntime` reads through, the
    fields of `ActorRuntime` that `new_actor_runtime` sets - is `pub(super)`.
  - `mod.rs` holds the description of the module, which it had none of, `Services` and the
    re-exports; every path into the module stays as it was.
