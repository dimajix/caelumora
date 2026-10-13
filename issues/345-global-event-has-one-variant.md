# 345 — `GlobalEvent` is an enum with a single variant

**Status:** done
**Area:** Code structure

## The problem

Since issue [335](335-channel-events-name-their-session.md), every event is about an entity, and
`GlobalEvent` (`caelumora-model/src/event.rs`) is left with the one variant
`Entity { kind, id, within, change }`. Whoever reads an event takes it apart with an irrefutable
`let GlobalEvent::Entity { … } = event` - the event bus (`read_action`, `part_readable`), the
workspace manager (`changes_who_may_run`) and the bus's tests -, and the JSON written for the
event streams wraps every event in `{"Entity": {…}}`, which the web UI unwraps
(`entityEvent` in `globalEvents.ts`).

## Why it matters

The enum promises a choice that is no longer there: a reader that matches on it looks as if other
forms could come, and a new kind of event looks as if it belonged in a variant of its own - the
very split issue 335 removed. The wrapper in the JSON is a level of nesting that says nothing.

## Considerations

- **A. Keep the enum.** It leaves room for an event that is about no entity - what instances of a
  deployment tell each other (issue [122](122-several-instances.md)), say. Costs nothing now, but
  the room is for something nobody has asked for, and readers keep the irrefutable `let`.
- **B. A struct `GlobalEvent { kind, id, within, change }`.** Readers take its fields; the
  constructors and `about` stay as they are, so most code does not change. The JSON loses the
  wrapper: `{"kind": "channel", "id": …, "within": …, "change": …}`, which the web UI reads
  directly - a change to the event streams that the web UI has to follow. Events are not stored,
  so no stored data is affected.
- **C. B, and the name `EntityEvent`.** Says what it is - an event about an entity -, but
  `GlobalEvent` is the name the event bus, the stream and the documents use (24 files of the
  workspace alone), and "global" still says something: these are the events of the bus, not the
  envelopes of a channel.

Recommendation: B. Should an event about no entity come, it is a decision of its own then.

## Done when

- The decision is recorded here; if B or C, `GlobalEvent` is a struct, the event bus, the
  workspace manager and the web UI read its fields, and `docs/EVENTS.md` and
  `docs/REST-SERVER.md` show its JSON.

## History

- **2026-10-13** — Opened as a follow-up of issue 335, which left `GlobalEvent` with one variant.
- **2026-10-13** — Decided: B. Done. `GlobalEvent` is a struct of public fields `kind`, `id`,
  `within` and `change`; its constructors and `about` are unchanged. The event bus (`read_action`,
  `part_readable`) and the workspace manager (`changes_who_may_run`) read its fields. The JSON of
  the event streams is the struct's own, without the wrapper. In the web UI, `GlobalEvent` is the
  type of that form, checked once as the global stream is read (`asGlobalEvent`, which replaces
  `entityEvent`), and its readers take its fields. `docs/EVENTS.md` and `docs/REST-SERVER.md` show
  the JSON. Events are not stored: no stored data has to be deleted.
