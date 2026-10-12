# 335 — The events of channels and their members are variants of their own

**Status:** open
**Area:** Code structure

## The problem

Step 5 of issue [323](323-kinds-enumerated-in-the-core.md) announced every kind whose events
name it by its own id as `GlobalEvent::Entity { kind, id, change }`: sessions, agents, users, user
profiles, workspaces, skills, artifacts, groups. The events of channels and of their members kept
variants of their own, since they name the session too: `ChannelCreated`, `ChannelUpdated`,
`ChannelDeleted`, `ChannelRestored`, `ChannelStarted`, `ChannelStopped`, `AgentChannelJoined`,
`AgentChannelLeft`, `AgentChannelStateChanged`, `UserChannelJoined` and `UserChannelLeft`
(`caelumora-model/src/event.rs`), each with the `SessionId` beside the `ChannelId`, by which the
web UI finds the channel in its session, and by which the event bus picks whom an event reaches
(`SubscriptionTopic::Channel`).

## Why it matters

`GlobalEvent` has two forms for one concept. As long as only the core's channels lie in a session,
that costs a second `match` in the event bus and the web UI. It matters once a plugin's kind lies
in something - an artifact published in a channel (issue
[328](328-artifacts-in-a-crate-of-their-own.md)) -: `Entity` cannot say where, so the plugin
either announces it without that, or needs a variant the core would have to know.

## Considerations

- **A. Keep the variants.** Channels and their members are the core's for good; the variants say
  exactly what they mean. A plugin kind that lies somewhere says so in its update's data
  (`EntityChange`), which the web UI reads per kind.
- **B. `Entity` names where it lies**: `within: Option<Scope>` - the scope of a channel names its
  session and itself -, filled by every kind that lies in one, read by the event bus for its topics
  and by the web UI for its lists. Channels become `Entity` of the kind `channel` with
  `within: SessionScope`, the members' events custom changes (`EntityChange::Custom`). One form for
  all, at the cost of a change to every reader of these events and to the web UI, and of the
  member's id moving into the change's data.
- **C. The channel's id alone**, the reader looking up its session. Rejected beforehand: the web
  UI would have to know every channel of every session, and the event bus decides by the session.

Recommendation: A until a plugin needs to say where its entity lies; then B, decided with that
plugin - artifacts are the likely first.

## Done when

- The decision is recorded here; if B, the events of channels and members are `Entity` events,
  the event bus and the web UI read them, and `docs/EVENTS.md` describes it.

## History

- **2026-10-12** — Opened as a follow-up of issue 323 (step 11): the variants that stayed in its
  step 5.
