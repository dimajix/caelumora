# 91 — `ChannelLoop` keeps its actors in two tables synchronised by a queue

**Status:** done
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
- **2026-09-29** — Done in part, as agreed with the maintainer; the rest
  deliberately not.
  - Done: `ChannelLoop::adopt(actor)` is the one way an actor enters the
    channel - moderator, table, mentions. `new` builds the loop first,
    with no actors, and adopts each participant's; `add_participant`
    adopts the newcomer's. The `Arc` around `ChannelLoop::state` is gone.
    `run`'s map is called `running`, and `ActorHandle` says what it is.
  - Not done: one table instead of two. They have different roles:
    `actors` says who takes part and is read from anywhere; `run`'s map
    holds the tasks and inboxes, whose tasks `run`'s `JoinSet` owns so
    that they end when the channel's task is aborted (a session stopped,
    a channel deleted), and the queue tells that owner what to start and
    stop. One table would need the tasks spawned outside `run`, a drop
    guard in `run` to abort them, and a lock with a "running" state so
    that a join during the start is neither started twice nor missed -
    more, and subtler, machinery than the queue it replaces. The same
    reasoning kept `SessionLoop`'s start queue in
    [116](116-session-loop-channel-tables.md).
