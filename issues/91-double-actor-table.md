# 91 — `ChannelLoop` keeps its actors in two tables synchronised by a queue

**Status:** open
**Area:** Code structure

## The problem

`ChannelLoop::actors: ArcSwap<HashMap<ActorId, Arc<dyn Actor>>>`
(`caelumora-runtime/src/channel.rs:65`) and, inside `run`, a local
`HashMap<ActorId, ActorHandle>` with the sender and abort handle (`:498`).
They are kept in step through `changes_tx`/`ActorChange` (`69-70, 526-542`)
with a special case for "joined before run started" (`529-533`). The
sequence that adopts an actor (`moderator.add_actor`, `actors.insert`,
`rebuild_mentions`) is written twice (`105-116`, `245-252`).

## Why it matters

Two tables for one set of actors, plus a queue to reconcile them, is the
kind of machinery that hides ordering bugs. The adopt sequence written
twice is one already waiting.

## Considerations

- First, cheap: `fn adopt(&self, actor)` for the duplicated sequence.
- Then: put the `ActorHandle` (sender, abort handle) into the `ArcSwap`
  entry, and the queue, `apply_change` and the special case go. Price: the
  `JoinSet` becomes `tokio::spawn` + `AbortHandle`, and the end criterion
  "last actor gone" (`513-517`) needs a replacement - in practice a channel
  ends only with its session today, so the criterion may simply go.
- `ChannelLoop::state: Arc<Mutex<…>>` is never cloned; the `Arc` can go
  with it.

## Done when

`ChannelLoop` has one table of actors and no change queue.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.1.9).
