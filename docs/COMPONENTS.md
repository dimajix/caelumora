# Components

An agent's behaviour is assembled from components: what tools it has, what
its prompt contains, how its context is built and how its tool calls run.
This document describes how components are declared, registered, wired and
built, and lists the built-in ones. The tools and commands that components
contribute are described in [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md);
how the agent uses its components during a turn is in
[AGENT-TURN.md](AGENT-TURN.md).

## Configuration, registry and container

*   **`ComponentConfig`** declares the components of a personality (or of a
    session's commands). Their order matters: it is the order in which they
    are handed out.
*   **`ComponentDescriptor`** configures one component: its `id`, its
    `kind`, its `params`, its `tags` and its `fail_policy` (`FailOpen`, the
    default, or `FailClosed`), which decides whether a failure of the
    component is skipped or fails the turn.
*   **`ComponentRegistry`** holds a factory per kind, together with the
    facets that kind provides. `ComponentRegistry::built_in` registers
    every built-in kind; the binaries hand it to the `RuntimeManager` as it
    is, which checks it (`verify`) and applies `component_defaults`. A test
    registers kinds of its own in it first - a stand-in for the model, say
    (issue 130).
*   **`ComponentContainer`** builds the components of a `ComponentConfig`
    and hands out their facets.

A descriptor of the reserved kind `ref` (`PRECONFIGURED_REF_KIND`) builds
no component of its own: its `params` are `{ "id": "..." }`, naming a
preconfigured descriptor stored in the `EntityStore`. It is read afresh
every time it is built, in the context of the graph that refers to it, so
secrets are resolved as that caller; the price is that the component is not
shared between references (a preconfigured `mcp_tool` discovers its tools
again for every personality that uses it).

## Facets

A component offers one or more *facets*: traits it implements to take part
in the agent loop.

| Facet | What it contributes | Where the trait lives |
|---|---|---|
| `ToolProvider` | tools the model may call | `crate::tool` |
| `ToolExecutor` | how the agent runs its tool calls | `crate::tool` |
| `CommandProvider` | user commands | `crate::command` |
| `PromptProvider` | prompt parts | `crate::context` |
| `ContextProvider` | what the model is given | `crate::context` |
| `LLMClient` | the language model the agent talks to | `crate::llm` |
| `PerceiveInProcessor` | handling of incoming messages | `crate::message` |
| `EmitOutProcessor` | handling of outgoing messages | `crate::message` |

`EmitOutProcessor` has no implementation yet; it is kept as an extension
point (issue 99). Each facet trait lives next to the code that uses it, not
among the implementations (issue 53).

## Wiring

Components are wired the way Spring Boot wires beans (issue 99).

*   **A kind declares what it provides.** It is registered as
    `ComponentKind::new(KIND).provides::<dyn ToolProvider>()…`, the
    counterpart of a bean's class. The components providing a facet are
    thus known before any of them is built, and the container checks that
    each factory kept to what its kind declared.
*   **A component asks for what it needs, by type.** Its factory calls
    `context.all::<dyn PromptProvider>().await` (or `one::<T>()`). This
    builds the components that provide the facet on demand, recursively,
    and returns every *other* one in the order of the configuration, each
    with its id and fail policy.
*   **Nothing is wired by hand.** There are no `deps`, so no dependency can
    be forgotten.
*   **Cycles are refused.** If a component that is still being built
    provides what is asked for, that is a real cycle, refused as
    `ComponentError::Cycle` naming both components.
*   **The order of the configuration counts.** The components nobody asks
    for are built in that order, and `get_all` hands them out in it.

### Facets of which there is exactly one

Of some facets an agent has exactly one: the `ContextProvider` (issue 98),
the `ToolExecutor` (issue 127) and the `LLMClient` (issue 130). They are
registered with
`ComponentRegistry::single::<T>(default_kind)` in
`component_impl::register_all`.

*   Two components of one container that provide such a facet are refused
    before anything is built.
*   If none does, a default is built the first time the facet is asked for
    (`ComponentContainer::one`, `ComponentContext::one`). A container that
    never asks - a session's commands, say - never builds one.
*   The default comes from, in this order: the personality's own
    component, the configuration (`AppConfig::component_defaults`, one
    component per such facet, checked when the `RuntimeManager` is made,
    see [CONFIGURATION.md](CONFIGURATION.md)), and finally the kind
    registered in code.

### Factories and tags

*   **Factories may wait.** `register_async` takes a factory that returns a
    future - for a remote system (`mcp_tool` discovers its tools without
    blocking a thread) or for other components. `register` takes one that
    does not.
*   **Tags** are kept on components, and `get_all` can filter by one,
    although nothing reads them yet.

## What a component may reach

`ComponentContainer::from_config` takes the registry and a
`ComponentContext`. The context holds:

*   the `SecurityContext` of the whole graph - for an agent's components,
    the agent's, in whose name they ask,
*   the `SecurityManager`, for the questions a component asks in that
    context (`AccessQuestions`; the memory asks what it may recall, issue
    131),
*   the `SecretManager`,
*   the `SessionReader` of the actor the components are built for
    (`session_reader::detached()` for components of no actor, such as the
    session's commands) - see [MEMORY.md](MEMORY.md) for what it reads,
*   and, internally, the graph being built, through which each component
    reaches the others it asks for.

Components are not given their actor's `ActorRuntime`
([128](../issues/128-actor-runtime-in-components.md)): through it they could
store the actor's state or act as the actor past the `Turn` they are called
with. What a component may use at build time is exactly what
`ComponentContext` holds. A new need gets its own narrow handle there, as
reading other channels did.

## Writing a component

The built-in components live in `caelumora-runtime/src/component_impl/`,
**one file per component kind**, named after the kind
(`calculator_tool.rs`, `notes.rs`, ...). Each file holds the kind constant,
its `register` function and its tests; `component_impl::register_all`
registers them all (issue 52). They are sorted by component, not by facet,
because one component may offer several facets - `create_plan_tool` and
`notes` provide tools and a prompt part. A new component gets a file of its
own and a line in `register_all`; `component_impl` holds components only.

They follow one convention (issue 101):

*   **Kind constant:** the kind in capitals with `_KIND`
    (`CALCULATOR_TOOL_KIND`). Only the command kinds live in the model,
    because `AppConfig`'s default names them.
*   **Struct:** the kind in CamelCase, with `Provider` for a `_tool` or
    `_command` kind (`CalculatorToolProvider`) and `Component` for a feature
    (`NotesComponent`, `ChannelHistoryComponent`).
*   **Names:** every tool and command name is a constant
    (`CALCULATE_TOOL_NAME`, `HELP_COMMAND_NAME`).
*   **Construction:** the factory fills `facets` and builds the struct
    literally. There is a `new` only where something is checked, as in
    `DefaultToolExecutor::new`.
*   **Parameters:** the turn is always the parameter `turn`.

What components share lives beside the facet: `tool::text_argument` and
`id_argument` for tool parameters, `context::render_sections` for a prompt
part that lists entries, and `session_reader::ReadError`, which a tool
passes on with `?`.

## Built-in components

| Kind | Facets | Params | What it does |
|---|---|---|---|
| `echo_tool` | `ToolProvider` | - | The tool `echo`, for testing. |
| `calculator_tool` | `ToolProvider` | - | The tool `calculate`. |
| `stop_tool` | `ToolProvider` | - | The tools `stop`, `bye` and `silence`, with which the agent ends its turn. |
| `create_plan_tool` | `ToolProvider`, `PromptProvider` | - | The tool `create_plan` and a system prompt part telling the model when to plan. |
| `mcp_tool` | `ToolProvider` | `url`, `auth`, `timeout_secs` (10), `requires_approval` (`false`) | The tools of a remote MCP server. |
| `notes` | `ToolProvider`, `PromptProvider` | - | Public notes of the agents in a channel: `note`, `revoke_note`, `read_notes` and a prompt part ([MEMORY.md](MEMORY.md)). |
| `private_notes` | `ToolProvider`, `PromptProvider` | - | The agent's private notes: `note_private`, `revoke_private_note` and a prompt part ([MEMORY.md](MEMORY.md)). |
| `agent_memory` | `ToolProvider`, `PromptProvider` | - | What the agent remembers across channels and sessions: `remember`, `forget` and a prompt part ([MEMORY.md](MEMORY.md)). |
| `channel_history` | `ToolProvider` | - | The tool `read_history` for other channels of the session ([MEMORY.md](MEMORY.md)). |
| `static_system_prompt_provider` | `PromptProvider` | `prompt` (required), `position` (`start` or `before_turn_input`, default `start`) | A fixed prompt part. |
| `state_context_provider` | `ContextProvider` | - | The default context provider: joins the prompt parts by position around the transcript ([AGENT-TURN.md](AGENT-TURN.md)). |
| `ollama_client` | `LLMClient` | `base_url` (`http://127.0.0.1:11434`), `model` (`gemma4:26b`), `think`, `reasoning_history` (`current_turn`), `system_messages` (`only_at_start`) | The default model: a model served by Ollama ([CONFIGURATION.md](CONFIGURATION.md#the-model)). |
| `default_tool_executor` | `ToolExecutor` | - | The default executor: runs a call on the `ToolProvider` that has the tool ([TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md)). |
| `transcript_appender` | `PerceiveInProcessor` | - | Appends incoming messages to the agent's transcript. |
| `state_history_appender` | `PerceiveInProcessor` | - | Appends incoming messages to the agent's history. |
| `help_command` | `CommandProvider` | - | The user command `/help`. |
| `wakeup_command` | `CommandProvider` | - | The user command `/wakeup`. |

The tools and commands themselves are listed with their parameters in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#built-in-tools-and-commands).
