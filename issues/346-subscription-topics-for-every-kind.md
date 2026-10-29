# 346 — The event bus has topics only for the kinds of the core

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Code structure
**Summary:** The event bus has subscription topics only for sessions, channels, agents and users, so a plugin cannot follow a single entity - an artifact, say - without reading every event.

## The problem

`SubscriptionTopic` (`caelumora-runtime/src/event.rs`) is a closed enum of `All`, `Session`,
`Channel`, `Agent` and `User`, and `EventBus::dispatch` picks an event's topics with a chain of
`event.about::<…>()` over exactly these kinds. Every other kind - a workspace, a skill, an
artifact, a group, any kind of a plugin - has no topic of its own; the comment in `dispatch` puts it
off until something follows one of them (issues [253](253-workspace.md),
[22](22-skill-management.md), [271](271-publishing-artifacts.md),
[286](286-groups-as-subjects.md)).

Whoever wants to follow a single artifact, or all workspaces, has two ways today, and both deliver
the whole stream to be sorted by the receiver:

- `subscribe_all()` and filter by `event.about::<Artifact>()` and the id - without any check of
  who may read it. The workspace manager does so for the kinds that change who may run a command
  (`changes_who_may_run` in `caelumora-runtime/src/workspace.rs`).
- `subscribe_as(context)`, which hands on everything the context may read, and filter the same
  way. A topic cannot narrow it: `follow` deliberately does nothing for such a subscription.

There is no use case for following a kind of a plugin yet; this issue is opened so that one can
refer to it when it comes.

## Why it matters

A plugin cannot follow its own entities, or those of another plugin, the way the core follows a
session: it reads every event of the deployment to find the few it wants. Adding a topic for a
plugin's kind would mean adding a variant about that kind to the core, which
[PLUGINS.md](../docs/PLUGINS.md) rules out. And since issue
[345](345-global-event-has-one-variant.md) every event has the one form `{ kind, id, within,
change }`, so the `match` over the core's kinds in `dispatch` is no longer needed to find what an
event is about.

## Considerations

- **A. Leave it.** Filtering at the receiver works and costs nothing until a subscriber reads many
  events it does not want. Plugins remain without topics.
- **B. A generic topic `Entity { kind: EntityKind, id: String }`.** `dispatch` derives the topics
  from the event's fields: its own entity, and the entity it lies in from `within` (so a channel's
  events reach its session, as now, from `Stored::Within` rather than a `match`). A typed
  constructor, `SubscriptionTopic::entity::<T>(&id)`, keeps the callers' ids typed;
  `subscribe_session`, `_channel`, `_agent` and `_user` could fold into it. To settle: that a
  `user_profile` event reaches the `User` topic today - its kind differs from the topic's, so
  either the profile names its user as `within`, or a subscriber names both kinds.
- **C. B, and a topic for a whole kind, `Kind(EntityKind)`.** For a subscriber that follows every
  entity of some kinds, like the workspace manager. It replaces the filter by kind, not the one by
  change.
- **D. Topics for `subscribe_as` as well.** Today a subscription made as someone receives
  everything they may read. A plugin that follows something in someone's name - the natural case
  for a plugin - would rather name the topics too, and the dispatcher hands on what matches *and*
  may be read. Independent of B and C, but only useful with them.

Recommendation: decide when the first use case comes; B is the shape that suits the form events
have now, and D is what that use case will most likely need.

## Done when

- The decision is recorded here; if B, the topics of an event are derived from its fields, a
  plugin can subscribe to one of its entities without the core knowing its kind, and
  `docs/EVENTS.md` describes the topics.

## History

- **2026-10-14** — Opened after the question whether a plugin can follow a single artifact on the
  event bus. There is no use case yet; opened to be referred to.
- **2026-10-29** — Issue [504](504-bus-has-two-delivery-paths.md) made every subscription one
  kind: `subscribe(context, topics)`, served by one dispatcher, and `follow` works for each. Option
  D - topics for a subscription in someone's name - is there now. The topics of an event are
  derived in one place, `SubscriptionTopic::of`, still by a `match` over the core's kinds; the
  shortcuts left are `subscribe_all`, `subscribe_session` and `subscribe_channel`. What remains
  here is the shape of the topics: B and C.
