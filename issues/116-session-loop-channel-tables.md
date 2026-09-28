# 116 — `SessionLoop` holds its channels three times

**Status:** open
**Area:** Code structure

## The problem

`SessionLoop` keeps `channels: DashMap<ChannelId, Arc<ChannelLoop>>`, the
running tasks in `tasks: std::sync::Mutex<HashMap<…>>`, and a
`start_tx/start_rx` queue through which a channel created while the loop
runs reaches `run`. Three structures describe the one set "my channels",
and `add_channel`/`remove_channel` have to update each.

## Why it matters

The same shape as the channel's double actor table
([91](91-double-actor-table.md)): two tables and a queue to keep them in
step. A channel that is in one and not the other is a bug nobody can see
from the types.

## Considerations

One `DashMap<ChannelId, ChannelEntry { loop: Arc<ChannelLoop>, task:
Option<AbortHandle> }>`; starting a channel fills `task`, stopping takes
it. The queue goes if `run` no longer needs to be told - the same trade as
in 91, and best done with it.

## Done when

`SessionLoop` has one field that knows which channels it has and which are
running.

## History

- **2026-09-28** — Split off from [97](97-small-model-unifications.md)
  (structure review, section 6 "Locks und Clones").
