# 104 — Two identical SSE builds, and a channel stream's membership loop lives in the server

**Status:** done
**Area:** User interaction
**Epic:** [74](74-refactoring-epic.md)
**Summary:** Both SSE handlers build their stream the same way, and the channel stream's membership loop is runtime knowledge in a handler.

## The problem

`global_events` (`routes/events.rs:17-43`) and `join_channel` (`routes/channels.rs:192-257`) build
their streams the same way: subscribe → optional snapshot → resync closure → `resync_on_lag` →
`chain` → `bounded_by_token`. `snapshot_event` exists twice with the same body (`events.rs:97`,
`channels.rs:187`); `serde_json::to_string(..).unwrap_or_default()` three times (`sse.rs:104`,
`events.rs:99`, `channels.rs:189`) and sends an empty `data:` on failure without a word.

`join_channel` additionally subscribes to the event bus itself
(`event_bus().subscribe_channel(cid)`), keeps a `Weak<ChannelLoop>` for the resync, and ends the
stream with a 25-line `take_until` loop that filters `UserChannelLeft` for the viewer and, on
`Lagged`, checks `actor_by_id(me)`. Issue [73](73-watch-leaving-duplicates-events.md) decided that
the channel has no leave notification of its own; the cost of that decision is this loop, which is
runtime knowledge (when does the channel stop a user's actor?) written in a REST handler.

## Why it matters

Nine steps in a handler for "give me the channel's events until I leave" is where the next subtle
bug goes; and the second stream builder will copy the first, bug included.

## Considerations

- `sse::stream(user, options, receiver, what, snapshot: impl Fn() -> Fut<impl Serialize>)` and
  `sse::named_event(name, &impl Serialize)`; both handlers become subscription + snapshot closure.
  `warn!` on a serialisation failure.
- `ChannelLoop::subscribe_as(ctx) -> Result<broadcast::Receiver<Envelope>>` that does the read check
  and closes when the user leaves the channel (a sender per `UserActor`, dropped when its actor
  stops). Handler: `open_channel` → `subscribe_as` → `sse::stream(…)`, ~12 lines. This is not a
  second broadcast of the leave - it is the natural end of a subscription - but 73 should be
  continued with it.

## Done when

- One function builds an SSE stream; both handlers use it.
- `join_channel` neither touches the event bus nor holds a `Weak`.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 5.2.3 and 5.4.1).
- **2026-09-29** — Done, as decided with the maintainer. One builder: `sse::stream(user,
  with_snapshot, receiver, what, snapshot, until)` - snapshot, resync on lag, end at the token's
  expiry or when `until` completes; `sse::named_event` replaces both `snapshot_event`s, and an event
  that does not serialize is a `warn!` instead of a silent empty `data:`. Not
  `ChannelLoop::subscribe_as` as considered: after issue 90 the maintainer's rule is that a user
  talks to their actor, never to the channel, and that only a participant follows a channel live
  (later also a read-only one, issue [124](124-observer-participants.md); an invisible admin with a
  way of their own is issue [125](125-invisible-admin.md)). So the channel stream is
  `UserActor::watch()`: the channel's broadcast, ended by a token the actor cancels when it stops
  (leaving) and when its task ends (a guard, so also when aborted). `join_channel` is now
  `channel_of` → `user_actor_of` → `watch` → `sse::stream`: no event bus, no `Weak`, no membership
  loop. On the way: the `UserActor`'s loop did not end when it left a channel (its own queue never
  closes; the 5-second watchdog aborted it) - it now ends with its inbox or when stopped; and
  deleting a channel stops its actors explicitly (`ChannelLoop:: stop_actors`), since actors whose
  tasks never ran would otherwise never end their watches. A reader who is no participant gets `403`
  on the stream and still reads the history; the web UI fetches the channel first, follows it live
  only as a participant, and otherwise shows the history with a hint instead of the input. Checked
  in the browser (local login): a message and an `@`-mentioned agent's answer arrive live; after
  removing the user over REST the view turns to history-only, also on a fresh load, without console
  errors.
