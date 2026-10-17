# 335 — The events of channels and their members are variants of their own

**Status:** done
**Area:** Code structure
**Summary:** The events of channels and their members are variants of their own beside `GlobalEvent::Entity`, which cannot say where an entity lies, so a plugin's kind that lies in a channel has no way to announce it.

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

Weighed later (2026-10-13):

- **D. A composite `ChannelAddress` as the channel's `Stored::Id`.** The address of a channel - its
  session and itself - would be what it is kept and found by, so `Entity` would carry the session
  in the id, and the event bus would read it with `about::<Channel>()` as it reads sessions,
  agents and users today. Rejected: the channel's id stays its `ChannelId` alone, and lookups go
  by it. A composite type of session and channel is a simplification of the signatures and of
  `Scope::ChannelScope` that carry both today - a refactoring of its own, not a solution here.
- **D′. A second associated type for what events name an entity by**, `ChannelAddress` for
  channels and the id for everyone else. Rejected: every kind would have two notions of its id,
  only so that channels can send their session along.
- **B with `within: Option<Scope>`**, as B describes it. Rejected: `Scope` is a name space - where
  something is found by name (`caelumora-model/src/scope.rs`) -, not what encloses an entity; the
  two only happen to coincide for a channel. And as an enum it leaves open at run time what the
  kind fixes: a channel always lies in a session, never in a user's scope.

## Decision

**B′: every kind names the kind of entity that encloses it, and `Entity` carries the enclosing
entity's id** (2026-10-13).

- A kind lies in exactly one kind of entity or in none. `Stored` gets `type Within: Enclosing`:
  `Channel` lies in a `Session`, later an artifact in a `Channel`; a kind that lies in nothing
  names `NoEntity`.
- `Enclosing` asks only for the id: `trait Enclosing { type Id: … }`, implemented for every
  `T: Stored` by a blanket impl (`type Id = T::Id`) and for `NoEntity` by an impl of its own.
  Coherence allows both, since the trait and `NoEntity` are both `caelumora-model`'s. `NoEntity`
  is no dummy `Stored`: it has no kind, no fields and nothing that could be deleted.
- `GlobalEvent::Entity { kind, id, within: Option<String>, change }`. Like `id`, `within` is a
  string, since the core does not know the id types of the plugins' kinds; the constructors
  (`created::<T>`, …) take it typed as `<T::Within as Enclosing>::Id`, and `about::<T>()` parses it
  back beside the id. A reader that knows the kind knows what `within` is the id of.
- The events of channels become `Entity` of the kind `channel` with `within` naming their session.
  The events of their members - joined, left, the state of an agent in the channel - become
  custom changes of the channel (`EntityChange::Custom`) with the member's id in their data.
- The event bus picks the topics of a channel's events from `about::<Channel>()`: `Session` from
  `within`, `Channel` from the id. `AgentChannelStateChanged` loses the `Agent` topic; nobody
  subscribes to it for that event - the `SessionActor`, which follows its agents' topics, receives
  it by `Session`.
- The event does not carry the kind of the enclosing entity: whoever knows the kind knows it from
  `T::Within`. Should a reader that does not know the kind need it - the web UI listing everything
  in a channel, say -, an `EntityKind` beside `within` is added then.

The cost: every `Stored` impl names its `Within` (stable Rust has no defaults for associated
types), and every reader of the channel and member events changes - the event bus, the
`SessionActor`, the tests, and `globalEvents.ts` in the web UI. Stored data is unaffected: events
are not stored.

## Done when

- The decision is recorded here; if B, the events of channels and members are `Entity` events,
  the event bus and the web UI read them, and `docs/EVENTS.md` describes it.
- With B′ decided: `Stored::Within` and `Enclosing` exist, `GlobalEvent` has no variants of its own
  for channels and their members, the event bus and the web UI read the `Entity` events of
  channels, and `docs/EVENTS.md` and `docs/STORAGE.md` describe `within`.

## History

- **2026-10-12** — Opened as a follow-up of issue 323 (step 11): the variants that stayed in its
  step 5.
- **2026-10-13** — Decided: B′. Every kind names the kind of entity that encloses it
  (`Stored::Within`, bounded by a new trait `Enclosing`, `NoEntity` for none), and `Entity`
  carries the enclosing entity's id as `within`. A composite `ChannelAddress` as the channel's id
  (D), a second id type for events (D′) and `within: Option<Scope>` were weighed and rejected;
  `ChannelAddress` as a simplification of signatures and of `Scope::ChannelScope` is left to an
  issue of its own.
- **2026-10-13** — Done. `Enclosing` came out differently from the decision in one point: instead
  of only the enclosing entity's id (`type Id`), it gives the whole `Address<I>` of an entity that
  lies in it - `(T::Id, I)` for a stored kind, `I` alone for `NoEntity` - and how an event writes
  and reads it (`write`, `read`). So the constructors and `about::<T>()` take and answer one value,
  `Address<T>`: a session still by its `SessionId`, unchanged at every call site, a channel by
  `(SessionId, ChannelId)`; a channel announced without its session, or a session with one, cannot
  be made and is not read. The members' events are the channel's custom changes `joined` and `left`
  with the member's `ActorId` - agent and user alike -, `agent_state_changed` with the `AgentId`,
  and `started` and `stopped`. `GlobalEvent` is left with the one variant `Entity`.
