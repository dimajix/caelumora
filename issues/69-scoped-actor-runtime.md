# 69 — The ActorRuntime hands out more than an actor needs

**Status:** done
**Area:** Open architecture questions

## The problem

Every actor gets an `ActorRuntime`, and through it the process-wide services
unrestricted: the `ControllerManager` (controllers of any agent or session),
`AgentStates` (the state handle of any agent), the `StateStore` (every
state, other agents' private notes included), the `EntityStore`, the
`EventBus` (any event with any attributes), the full `SessionLoop` (creating
and deleting channels) and the full `ChannelLoop` (`cancel`, `wakeup`, ...).

What the actors actually use is almost only about themselves:

| Access today | Used by | Problem | Stricter |
|---|---|---|---|
| `controller_manager()` | agent and user at start | controllers of any agent or session | `new_agent_controller()` / `new_user_controller()` for the own actor only |
| `agent_states()` | agent at start | handle of any agent | `agent_state()`: the own handle only |
| `state_store()` | agent: its own `AgentChannelState` | reads and writes every state | load / store of the own agent-channel state only |
| `entity_store()` | agent: its own personality | the whole store, checked only by the security context | `personality()`, or only what is needed |
| `event_bus()` | agent, user, approver: publish, subscribe | any event with any attributes | specific announcements (e.g. `announce_channel_state_changed()`) and the own subscription |
| `session()` | only the `SessionReader` | the whole `SessionLoop` | not handed out; used inside the reader |
| `channel()` | agent, user, `SessionReader` | the whole `ChannelLoop` | a narrow view: `classification()`, `security_attributes()`, `may_read()` |
| `security_manager()`, `llm_client()`, `secret_manager()`, `component_registry()`, `new_tool_executor()`, `command_executor()` | building, checks | uncritical: services, or questions that change nothing | stay |

## Why it matters

- Components were to get the `ActorRuntime` through `ComponentContext`, as
  the right level to take services from. Handed out as it is, a component
  could read around the confidentiality rules - read another channel's
  history or an agent's private notes straight from the `StateStore` instead
  of through the `SessionReader`, which enforces `may_read` (rules of
  [61](61-reasoning-leaves-the-agent.md), principle 12 in
  [19](19-memory-tiers.md)). Components therefore get a narrow
  `SessionReader` instead for now ([65](65-generic-component-state.md)).
- Least authority: an actor, and a component of it, should hold capabilities
  for itself, not for the whole process.

## Considerations

- **The `ActorRuntime` as a set of capabilities for one actor:** made with
  the actor's identity (its `ActorId`, the `AgentId` for agents), it hands
  out only that actor's state, controllers and narrow views; the
  `SessionReader` becomes one of its methods (`session_reader()`).
- Once it is scoped, `ComponentContext` can carry the `ActorRuntime` instead
  of a `SessionReader`. Components of a session outside of any channel (the
  user commands) have no actor: an `Option`, or a `SessionRuntime` of their
  own.
- The users of the broad accessors are few (see the table); most are at the
  start of an actor, so the change is mostly construction.
- The same question may come up for `ChannelRuntime` and for what a
  `ChannelLoop` offers to actors.

## Done when

- An `ActorRuntime` gives access only to what concerns its actor, and can be
  handed to that actor's components without letting them read around the
  confidentiality rules.

## History

- **2026-09-28** — Issue created at the maintainer's request, while the
  first step of [65](65-generic-component-state.md) moved the
  `SessionReader` from `TurnContext` into `ComponentContext`: the maintainer
  proposed giving components the `ActorRuntime` instead, as the right level;
  handed out as it is, it would let components read around the
  confidentiality rules, so it is to be restricted to its actor first.
- **2026-09-28** — Decided with the maintainer:
  1. The `ActorRuntime` knows its actor: it is made with the `ActorId` and
     the ids of session and channel (`ChannelRuntime::new_actor_runtime` in
     `ChannelLoop::new`, where the channel is known - it is not attached yet
     when the actors are built).
  2. Broad accessors are replaced by own ones: `new_agent_controller` /
     `new_user_controller` (an error for anyone but the own actor) instead of
     `controller_manager()`; `agent_state()` (the own handle) instead of
     `agent_states()`; `load_channel_state()` / `store_channel_state()` for
     the own agent and channel instead of `state_store()`; `personality()`
     instead of `entity_store()`; `subscribe()` (the own subscription, by
     kind of actor as today) and `announce_channel_state_changed()` instead
     of `event_bus()`; `classification()` and `channel_attributes()` instead
     of `channel()` for what agents need. `security_manager`, `llm_client`,
     `secret_manager`, `component_registry`, `new_tool_executor` and
     `command_executor` stay.
  3. The full `ChannelLoop` and `SessionLoop` (`channel()`, `session()`) stay
     `pub(crate)`: the user actor needs the channel for commands, the
     `SessionReader` both. Components from other crates cannot reach them;
     built-in components could, and are held to it by review. Moving the
     reader into the runtime module for real enforcement was not wanted.
  4. `ComponentContext` carries `Option<ActorRuntime>` instead of the
     `SessionReader` - optional, since only actors have an `ActorRuntime`
     (the session's commands have none). Components build their reader from
     it (`ActorRuntime::session_reader()`); without a runtime they read
     nothing (`session_reader::detached()`).
- **2026-09-28** — Implemented as decided. `ActorRuntime` holds the actor's
  id and the ids of its session and channel; the broad accessors
  (`controller_manager`, `agent_states`, `state_store`, `entity_store`,
  `event_bus`) are gone, replaced by `new_agent_controller`,
  `new_user_controller`, `agent_state`, `load_channel_state`,
  `store_channel_state`, `personality`, `subscribe`,
  `announce_channel_state_changed`, `classification`, `channel_attributes`
  and `session_reader`; `channel()` and `session()` are `pub(crate)`.
  `ComponentContext` carries `Option<ActorRuntime>` instead of the reader
  (`ComponentContext::session_reader()` builds it, or `detached()` without a
  runtime). `AgentActor` no longer holds the `StateStore`;
  `TurnContext::store_channel_state` is gone - the agent stores through its
  runtime. Test `an_actor_runtime_acts_for_its_own_actor_only`. Not changed:
  the `ChannelRuntime` still offers `state_store`, `event_bus` and
  `controller_manager` to the `ChannelLoop` - the channel level, noted
  above as a question of its own.
- **2026-09-28** — `subscribe()` chose the topics by kind of actor, hidden
  (an approver fell into the catch-all arm); at the maintainer's question it
  became `subscribe(&[OwnTopic])` with `OwnTopic::Actor` and
  `OwnTopic::Channel`, so every caller says what it subscribes to - one
  subscription, since the user needs both - and still nothing about others
  can be subscribed to.
