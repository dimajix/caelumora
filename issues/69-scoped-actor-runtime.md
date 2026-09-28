# 69 — The ActorRuntime hands out more than an actor needs

**Status:** open
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
