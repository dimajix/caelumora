# 73 — `watch_leaving` duplicates what the event bus announces

**Status:** done
**Area:** Sessions and channels
**Summary:** `ChannelLoop::watch_leaving` is a second channel for the fact that someone left, which the `EventBus` already announces as `UserChannelLeft`/`AgentChannelLeft`.

## The problem

For [46](46-channel-participants.md), `ChannelLoop` got a broadcast channel of its own, `left_tx`,
exposed as `ChannelLoop::watch_leaving()`. It reports every participant that is removed. Its only
user outside tests is the REST server, which ends a removed user's event stream of the channel with
it (`take_until` in `join_channel`).

The same fact is already announced. When `ChannelController` stores the removal, it publishes
`GlobalEvent::UserChannelLeft` or `AgentChannelLeft` on the `EventBus`, with the channel's
attributes, and the bus delivers it on the topic `SubscriptionTopic::Channel(cid)`. The approver
already follows participants that way (`handle_global_event`).

So there are two mechanisms for one fact, and `ChannelLoop` carries an extra field and a public
method for it.

## Why it matters

- Everything else about membership is observed through the bus: the web UI (issue
  [35](35-sidebar-live-updates.md)), the approver, the session list. A second path is something the
  next reader has to discover and keep in step.
- It adds to the channel's public interface, which issue [71](71-access-helpers-on-entities.md) just
  slimmed down.

## Considerations

- **Use the bus in REST.** `join_channel` subscribes to `SubscriptionTopic::Channel(cid)` on
  `state.runtime_manager.event_bus()` and ends the stream on `UserChannelLeft(_, cid, me)`. Then
  `left_tx` and `watch_leaving` go away.
  - A topic subscription can fall behind (128 events) and report `Lagged`. On a lag the stream can
    check whether the user still takes part (`channel.user_actor(me)`), and end if not. Missing the
    event is therefore harmless.
  - The event is published when the removal is stored, before the channel stops the actor. For
    ending a stream that order is fine: the stream ends slightly earlier, never later.
  - When the stream reconnects, the channel is answered as absent (`channel_of`), so a user who was
    removed cannot come back that way either.
- **Keep `watch_leaving`.** Its only advantage is that it is local to the channel and needs no topic
  subscription. It does not seem worth a second mechanism.

Tentative recommendation: use the bus and remove `watch_leaving` and `left_tx`.

## Done when

- A removed user's channel stream ends through the membership events of the `EventBus`.
- `ChannelLoop` has no leave notification of its own.

## History

- **2026-09-28** — Issue created at the maintainer's request after [46](46-channel-participants.md)
  introduced `watch_leaving`.
- **2026-09-28** — Done as recommended: `join_channel` subscribes to the channel's topic on the
  event bus and ends the stream on the user's `UserChannelLeft`; on a lag it ends if the user no
  longer has an actor in the channel. `left_tx` and `ChannelLoop::watch_leaving` are gone.
- **2026-09-29** — Continued by [104](104-sse-stream-and-subscribe.md): the REST server no longer
  listens for `UserChannelLeft` to end a removed user's channel stream. The stream is followed
  through the user's actor (`UserActor::watch`) and ends with it; the announcement on the bus stays
  what it was, for everyone else.
