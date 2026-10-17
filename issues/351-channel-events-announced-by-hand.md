# 351 — The lifecycle events of a channel are announced by hand at ten places

**Status:** done
**Area:** Code structure
**Epic:** [349](349-core-review-epic.md)
**Summary:** The lifecycle events of a channel are announced at ten places, each assembling the key, the attributes and the classification by hand.

## The problem

An event about a channel is published as

```rust
publish_classified(GlobalEvent::…::<Channel>(&(session_id, channel_id)), channel.security_attributes.clone(), std::slice::from_ref(&channel.classification))
```

the key, the attributes and the classification assembled by the caller each time:

- `STARTED` and `STOPPED` in `session/mod.rs:333`, `:384` and `session/channels.rs:249`;
- `created` in `session/channels.rs:176` and `session/manager.rs:306`;
- `deleted` in `session/channels.rs:251` and `session/manager.rs:475`;
- `restored` in `session/channels.rs:319`;
- `AGENT_STATE_CHANGED` in `runtime.rs:264`;
- joins and leaves in `controller/channel.rs:208`, the one place that has a method for it
  (`membership_event`).

`delete_session` carries a `Vec<(ChannelId, SecurityAttributes, Classification)>` through the
deletion only to assemble the same call afterwards (`session/manager.rs:437`).

## Why it matters

What an event about a channel is announced with - its attributes for who may hear it, its
classification for who is cleared - is a rule of the channel, decided ten times by its callers. A
change to it, such as announcing to who reached the channel as it was ([312](312-order-and-reach-not-everywhere.md),
as `ChannelController::update` does with two classifications), has to be made ten times.

## Considerations

- One function in `channel`,
  `pub(crate) fn announce(events: &EventBus, channel: &Channel, event: GlobalEvent)`,
  that supplies attributes and classification; the ten callers build only the `GlobalEvent`. A
  `ChannelLoop::announce(event)` for the running channel beside it, which reads its document.
- Or the constructors on `GlobalEvent` that take the `Channel` and know its key, with the
  announcing still the callers' - less, since the attributes and classification stay with them.
- `delete_session` keeps the `Channel`s it loaded instead of taking them apart.

## Done when

- The attributes and the classification an event about a channel is published with are assembled in
  one place.
- No event changes its key, attributes or classification; the event tests of sessions and channels
  pass unchanged.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
- **2026-10-14** — Done more generally than considered, as decided with the maintainer: the same
  call stood for every classified kind - workspaces seven times, skills six, artifacts eight -, and
  the model knows both halves of it already (`Secured::security_attributes`,
  `Stored::classification`). So it is not a function of the channel but `Announcing::announce(event,
  &entity)` in `caelumora-model`'s `event`, a blanket extension of every `EventPublisher` as
  `Enforcement` is of every `SecurityManager`, so that the controllers' `dyn EventPublisher` has it
  too. The caller builds the event and its address; a `Stored::address()` would have cost every kind
  an implementation, for the one kind that lies in another.

  Eleven announcements of channels use it, among them `add_participant` and `announce_resting` of
  the `ChannelController`, which the list above missed, and those of workspaces, skills and
  artifacts in a commit of their own. `delete_session` keeps the channels it loaded. Assembled by
  hand stay those that reach differently, as their comments say: a reclassification, with the
  classification it had and the one it has (issue 312), a leave, to whoever left besides (issue 23),
  and a version of an artifact (`publish_classified_part`). The unclassified kinds - sessions,
  agents, users - still publish with `publish(event, attributes)`, some forty times; `announce`
  serves them as well, and moving them is left for when they are touched.
