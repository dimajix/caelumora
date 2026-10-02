# 128 — Components reach the whole ActorRuntime, including what only the actor may do

**Status:** done
**Area:** Open architecture questions

## The problem

`ComponentContext::actor_runtime` is a public field of type `Option<ActorRuntime>`. Every component
of an agent can therefore call every `pub` method of its actor's runtime - not only
`session_reader()`, which is what [69](69-scoped-actor-runtime.md) and
[65](65-generic-component-state.md) meant it for, but also:

- `store_channel_state` / `load_channel_state` - write the agent's state in its channel directly,
  past `Turn::commit_turn`;
- `agent_state` - the agent's own state handle, past `Turn::stage_agent_change`;
- `announce_channel_state_changed`, `subscribe_channel` - announce and receive events as the actor;
- `llm_client`, `component_registry`, `personality`, `command_executor` - the services the actor is
  built with.

No component uses any of them today: the only access is `ComponentContext::session_reader()`. Every
other caller is in `actor_impl` (agent, user, approver). Found while narrowing the public surface in
[107](107-public-interface.md).

## Why it matters

- **Commit, don't write** ([19](19-memory-tiers.md)): a turn's changes are staged and taken over
  when it completes, so that a failed turn leaves none. A component that stores the channel state
  itself bypasses that, and its write can be overwritten by - or overwrite - the actor's own.
- **Least authority** (69): the `ActorRuntime` was scoped to its actor so that it could be handed to
  components. It now keeps the actor's capabilities from other actors, but not from the actor's own
  components.

## Considerations

- **(a) Narrow `ActorRuntime`:** the methods only actors use become `pub(crate)`. Cheap, but it ties
  "what a component may do" to crate boundaries: a component written inside the runtime crate could
  still call them.
- **(b) Hand components a view, not the runtime:** `ComponentContext` keeps the `ActorRuntime`
  private and offers what components need as its own methods (today only `session_reader()`, perhaps
  `classification()` later). The component API is then written down in one place,
  `ComponentContext`, like the `Turn` API for turn time (65).
- **(c) Split the runtime:** an `ActorRuntime` for the actor and a narrower `ComponentRuntime` (or
  similar) derived from it for its components. Clearest in types, most code.
- Whichever is chosen, components of no actor (the session's commands) keep getting a detached
  reader, as today.

## Done when

A component cannot, through what it is given when it is built, write its actor's state or act as its
actor outside of the `Turn` it is called with.

## History

- **2026-09-29** — Opened while working on [107](107-public-interface.md): the `pub(crate)` pass
  left `ActorRuntime` alone, because narrowing it is a question of what components may do, not of
  visibility.
- **2026-09-29** — Done, with a variant of (b): `ComponentContext` holds no runtime at all, only
  `session_reader: Arc<dyn SessionReader>`, a public field like `security_context` and
  `secret_manager`. `AgentActor` passes `ActorRuntime::session_reader()`, components of no actor
  `session_reader::detached()`; the `Option` and its fallback are gone. What a component is given is
  then exactly what the struct holds - a new need gets its own narrow handle, as reading other
  channels did. (a) was not enough: every component lives in the runtime crate, so `pub(crate)`
  would have kept none of them out. (c) would have been this with an extra type around one field.
