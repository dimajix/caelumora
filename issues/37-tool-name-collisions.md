# 37 — Two tools of the same name: the first one silently wins

**Status:** done
**Area:** Tools and approvals

## The problem

`DefaultToolExecutor` looks tools up by name in its providers, in order,
and runs the first match. If two components of a personality provide a
tool of the same name, the second one is unreachable, and nothing says so:
the model is offered the name once (or twice, depending on how the tool
list is assembled) and every call goes to the first provider.

User commands treat the same situation as a configuration error
([28](28-user-commands.md)): `CommandExecutor::new` refuses two components
providing the same command and names both, and the session does not start.

## Why it matters

A collision is always a mistake in the configuration — two `mcp_tool`
components whose prefixes clash, a built-in tool shadowed by a remote one,
a personality that references the same preconfigured component twice
under different ids. Resolved by order, it shows up much later as an agent
that "ignores" a tool, or calls the wrong one with arguments meant for the
other — which is the hardest kind of error to trace back.

## Considerations

- **Same rule as for commands.** Detect at construction, report both
  component ids and the tool name, refuse. That needs the component ids,
  so `new_tool_executor` should take `Vec<ComponentImpl<dyn ToolProvider>>`
  instead of the bare facets — the same shape `CommandExecutor::new` has.
- **Consequence: a personality with colliding tools no longer starts.**
  The agent's construction fails with a `ComponentError` naming the
  components, so `ensure_session` answers `500` with that message. Stored
  personalities that collide today would surface then; that is the point,
  but it should be mentioned when this lands.
- **Tools discovered at run time.** `mcp_tool` discovers its tools once
  when built, so a collision is visible at construction as well. If
  providers ever gain tools later ([32](32-unavailable-tools-invisible-to-model.md) touches this), the check has to run
  again then.
- **`FailOpen` components.** A component that failed to build contributes
  no tools and therefore no collision; nothing special is needed.

## Done when

- Two components of one agent providing a tool of the same name are
  reported with both component ids and the tool name, and the agent does
  not start.
- `new_tool_executor` receives the providers with their component ids.

## History

- **2026-09-24** — Opened while aligning tools and commands (issue 28):
  commands refuse duplicate names, tools resolve them by order.
- **2026-09-24** — Done. `DefaultToolExecutor::new` and
  `ActorRuntime::new_tool_executor` take `Vec<ComponentImpl<dyn ToolProvider>>`
  and return a `ComponentResult`; a tool name provided by two components is a
  `ComponentError::InvalidConfig` naming the tool and both component ids,
  which ends `AgentActor::new` and so the session start. The check is the
  same one `CommandExecutor::new` makes. The personalities stored in
  `local/entity_store` and the ones the REST server and the TUI set up were
  checked: none collides, so nothing that started before fails now.
