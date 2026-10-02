# 116 — `SessionLoop` holds its channels three times

**Status:** done
**Area:** Code structure

## The problem

`SessionLoop` keeps `channels: DashMap<ChannelId, Arc<ChannelLoop>>`, the running tasks in `tasks:
std::sync::Mutex<HashMap<…>>`, and a `start_tx/start_rx` queue through which a channel created while
the loop runs reaches `run`. Three structures describe the one set "my channels", and
`add_channel`/`remove_channel` have to update each.

## Why it matters

The same shape as the channel's double actor table ([91](91-double-actor-table.md)): two tables and
a queue to keep them in step. A channel that is in one and not the other is a bug nobody can see
from the types.

## Considerations

One `DashMap<ChannelId, ChannelEntry { loop: Arc<ChannelLoop>, task: Option<AbortHandle> }>`;
starting a channel fills `task`, stopping takes it. The queue goes if `run` no longer needs to be
told - the same trade as in 91, and best done with it.

## Done when

`SessionLoop` has one field that knows which channels it has and which are running.

## History

- **2026-09-28** — Split off from [97](97-small-model-unifications.md) (structure review, section 6
  "Locks und Clones").
- **2026-09-29** — Done for the two tables: `SessionLoop::channels` is a `DashMap<ChannelId,
  ChannelEntry { channel, task: Option<AbortHandle> }>`; the `tasks` mutex is gone, and with it its
  "poisoned" error paths. `start_channel` sets the task while it holds the entry, so
  `delete_channel` - which removes the entry and aborts its task - finds either the task or no
  channel; a channel deleted while it waits to be started is never started, now by the entry's
  absence instead of a second lock. The start queue carries channel ids instead of loops - the entry
  is the one source. The queue itself stays: `run` owns the `JoinSet` that stops every channel with
  the session, and it has to be told about channels created later; dropping it is the same trade as
  in [91](91-double-actor-table.md). New test: a channel deleted before the session runs is deleted,
  never started and never stopped.
