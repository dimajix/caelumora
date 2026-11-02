# 528 — The session module re-exports the channel's types, which so have two paths

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `caelumora_domain::session` re-exports `Channel`, `ChannelId`, `ChannelState` and `Participant` from `caelumora_domain::channel`, so that each of them is imported by two paths - most code by the channel's, some by the session's.

## The problem

`caelumora-domain/src/session.rs` begins with

```rust
pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
```

The four types are defined in `channel` (since issue
[526](526-large-modules-of-the-domain.md) in `channel/mod.rs`, `channel/participant.rs` and
`channel/state.rs`), and are imported by both paths: `Participant` by `channel::` in 29 files and
by `session::` in 6 (in `caelumora-agents`, `caelumora-domain`, `caelumora-kernel` and
`caelumora-rest-server`), and so the others. A reader meets one type under two names and cannot
tell from an import where it lives; AGENTS.md asks for one name per concept and fewer ways of
doing the same thing. The re-export looks like what is left of a time when the channels were part
of the session's module.

Found in the review of the step `channel` of issue 526.

## Considerations

- **Drop the re-export.** Every import names `channel`, where the types are defined; a mechanical
  change of the imports that use `session::` for them.
- **A module of the participants' own.** `Participant` belongs to a session as much as to a
  channel, and could lie in a module of its own rather than in `channel`. That changes the path of
  every use, and belongs to the question of grouping the domain's flat modules, which issue 526
  left open - not to this issue.

Recommended: drop the re-export.

## Done when

- `Channel`, `ChannelId`, `ChannelState` and `Participant` are imported by one path, that of
  `channel`, and `session` re-exports none of them.

## History

- **2026-11-02** — Opened at the maintainer's request, from the review of the step `channel` of
  issue [526](526-large-modules-of-the-domain.md).
