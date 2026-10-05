# Components

An agent's behaviour is assembled from components: what tools it has, what its prompt contains, how
its context is built and how its tool calls run. A session's user commands and a channel's moderator
are components too. This document describes how components are declared, registered, wired and
built, and lists the built-in ones. The tools and commands that components contribute are described
in [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md); how the agent uses its components during a turn
is in [AGENT-TURN.md](AGENT-TURN.md).

## Configuration, registry and container

*   **`ComponentConfig`** declares the components of a personality, of a session's commands or of a
    channel (its moderator, issue 174). It is written as the list of its descriptors, directly
    under the key that holds it (`components`, `commands`, issue 201). Their order matters: it is
    the order in which they are handed out.
*   **`ComponentDescriptor`** configures one component: its `id`, its `kind`, its `params`, its
    `tags` and its `fail_policy` (`FailOpen`, the default, or `FailClosed`), which decides whether a
    failure of the component is skipped or fails the turn.
*   **`ComponentRegistry`** holds a factory per kind, together with the facets that kind provides.
    `ComponentRegistry::built_in` registers every built-in kind; the binaries hand it to the
    `RuntimeManager` as it is, which applies `component_defaults`. A test registers kinds of its own
    in it first - a stand-in for the model, say (issue 130).
*   **`ComponentContainer`** builds the components of a `ComponentConfig` and hands out their
    facets.

A descriptor of the reserved kind `ref` (`PRECONFIGURED_REF_KIND`) builds no component of its own:
its `params` are `{ "id": "..." }`, naming a preconfigured descriptor stored in the
`ComponentStore`. It is read afresh every time it is built, in the context of the graph that refers
to it, so secrets are resolved as that caller; the price is that the component is not shared
between references (a preconfigured `mcp_tool` discovers its tools again for every personality that
uses it).

## Facets

A component offers one or more *facets*: traits it implements to take part in the agent loop.

| Facet | What it contributes | Where the trait lives |
|---|---|---|
| `ToolProvider` | tools the model may call | `crate::tool` |
| `ToolExecutor` | how the agent runs its tool calls | `crate::tool` |
| `CommandProvider` | user commands | `crate::command` |
| `PromptProvider` | prompt parts, of agents and external agents alike (issue 204) | `crate::context` |
| `ContextProvider` | what the model is given | `crate::context` |
| `LLMClient` | the language model the agent talks to | `crate::llm` |
| `ExternalAgent` | the remote agent an external personality's agents are (issue 179) | `crate::external` |
| `PerceiveInProcessor` | filtering of incoming messages, of agents and external agents alike | `crate::message` |
| `EmitOutProcessor` | filtering of the messages an agent emits | `crate::message` |
| `Moderator` | how a channel routes its events and who is asked to respond | `crate::moderator` |
| `ResponderPolicy` | whom the `policy_moderator` asks when nobody was mentioned | `crate::moderator_impl` |

`PerceiveInProcessor` and `EmitOutProcessor` have no built-in implementation; they are kept as
extension points (issues 99, 196). A processor is given the messages and the actor's
`SecurityContext` and returns what is to be processed - it may drop, change or add messages - and
records nothing: the actor keeps all it was offered in its history, and what passes in its
transcript. Each facet trait lives next to the code that uses it, not among the
implementations (issue 53).

## Wiring

Components are wired the way Spring Boot wires beans (issue 99).

*   **A kind declares what it provides.** It is registered as
    `ComponentKind::new(KIND).provides::<dyn ToolProvider>()…`, the counterpart of a bean's class.
    The components providing a facet are thus known before any of them is built, and the container
    checks that each factory kept to what its kind declared.
*   **A component asks for what it needs, by type.** Its factory calls `context.all::<dyn
    PromptProvider>().await` (or `one::<T>()`). This builds the components that provide the facet on
    demand, recursively, and returns every *other* one in the order of the configuration, each with
    its id and fail policy.
*   **What is asked for is kept as it comes.** Whoever holds a component - an agent, a channel,
    another component - keeps the `ComponentImpl` that `one` or `all` returned, not just its
    `facet`. It dereferences to the facet, so the facet's methods are called on it directly
    (`self.moderator.route(…)`); `facet` is needed only to hand on the `Arc`. Id and fail policy are
    then at hand when a call fails, whether or not the component needs them today.
*   **Nothing is wired by hand.** There are no `deps`, so no dependency can be forgotten.
*   **Cycles are refused.** If a component that is still being built provides what is asked for,
    that is a real cycle, refused as `ComponentError::Cycle` naming both components.
*   **The order of the configuration counts.** The components nobody asks for are built in that
    order, and `get_all` hands them out in it.

### Facets of which there is exactly one

Of some facets an agent has exactly one: the `ContextProvider` (issue 98), the `ToolExecutor`
(issue 127) and the `LLMClient` (issue 130), an external agent exactly one `ExternalAgent` (issue
179); a channel has exactly one `Moderator` and one `ResponderPolicy` (issue 174). They are
registered with `ComponentRegistry::single::<T>(default_kind)` in the `register_all` of the
directory that holds the default kind (`component_impl`, `tool_impl`, `llm_impl`, `external_impl`,
`moderator_impl`).

*   Two components of one container that provide such a facet are refused before anything is built.
*   If none does, a default is built the first time the facet is asked for
    (`ComponentContainer::one`, `ComponentContext::one`). A container that never asks - a session's
    commands, say - never builds one.
*   A single may have no default (`single::<T>(None)`) where none makes sense: of `ExternalAgent`,
    the remote agent is always named. A container that is asked for it and provides none fails then,
    as it does if a default kind is not registered as providing the facet - nothing is checked
    beforehand.
*   What a container's single is, is known without building it: `ComponentRegistry::kind_of` names
    the kind of the component that provides it, or of the default - `None` if the container refers
    to a preconfigured component, which might provide it.
*   The default comes from, in this order: the container's own component, the defaults the
    container was built with (`ComponentContainer::with_defaults`, issue 214 - an external agent's
    actor gives its `ContextProvider` so, `external_context_provider`), the configuration
    (`AppConfig::component_defaults`, one component per such facet, checked when the
    `RuntimeManager` is made, see [CONFIGURATION.md](CONFIGURATION.md)), and finally the kind
    registered in code. `kind_of` knows only the registry's: it is asked of channels, which are
    built without defaults of their own.

### Factories and tags

*   **Factories may wait.** `register_async` takes a factory that returns a future - for a remote
    system (`mcp_tool` discovers its tools without blocking a thread) or for other components.
    `register` takes one that does not.
*   **Tags** are kept on components, and `get_all` can filter by one, although nothing reads them
    yet.

## What a component may reach

`ComponentContainer::from_config` takes the registry and a `ComponentContext`. The context holds:

*   the `SecurityContext` of the whole graph - for an agent's components, the agent's, in whose name
    they ask; for a channel's, its owner's,
*   the `SecurityManager`, for the questions a component asks in that context (`AccessQuestions`;
    the memory asks what it may recall, issue 131),
*   the `SecretManager`,
*   nothing of the session: what a component reads of it - the other channels, their notes and
    histories, what its agent may recall - it reads through the `Turn` it is called with
    (`Turn::session`, issue 163; see [MEMORY.md](MEMORY.md#reading-other-channels)),
*   and, internally, the graph being built, through which each component reaches the others it asks
    for.

Components are not given their actor's `ActorRuntime`
([128](../issues/128-actor-runtime-in-components.md)): through it they could store the actor's state
or act as the actor past the `Turn` they are called with. What a component may use at build time is
exactly what `ComponentContext` holds. A new need gets its own narrow handle there, as reading other
channels did.

## Writing a component

The built-in components live in `caelumora-runtime/src/`, **one file per component kind**, named
after the kind (`calculator_tool.rs`, `notes.rs`, ...), in the directory of the facet that is their
purpose:

| Directory | Components |
|---|---|
| `tool_impl/` | tool providers and the `ToolExecutor` (`calculator_tool`, `mcp_tool`, `default_tool_executor`, ...) |
| `command_impl/` | command providers (`help_command`, `wakeup_command`) |
| `llm_impl/` | language models (`ollama_client`) |
| `external_impl/` | the ways to reach an external agent (`a2a_agent`, issue 179) - a module per kind: `a2a_agent/` holds the component (`mod.rs`), the protocol's types (`wire.rs`), their translation to and from `crate::external` (`translation.rs`) and the transport (`client.rs`, issue 210) |
| `moderator_impl/` | the moderator and its responder policies |
| `component_impl/` | everything else, and every component that offers several facets |

A component that offers several facets - `create_plan_tool`, `notes` and the other memory components
provide tools and a prompt part - stays in `component_impl`, because it cannot be sorted by one
facet. Each file holds the kind constant, its `register` function and its tests; each directory's
`register_all` registers its kinds and the singles it holds the default for, and
`ComponentRegistry::built_in` calls them all. A new component gets a file of its own and a line in
its directory's `register_all`; the `*_impl` directories hold components only.

They follow one convention (issue 101):

*   **Kind constant:** the kind in capitals with `_KIND` (`CALCULATOR_TOOL_KIND`). Only the command
    kinds live in the model, because `AppConfig`'s default names them.
*   **Struct:** the kind in CamelCase, with `Provider` for a `_tool` or `_command` kind
    (`CalculatorToolProvider`) and `Component` for a feature (`NotesComponent`,
    `ChannelHistoryComponent`).
*   **Names:** every tool and command name is a constant (`CALCULATE_TOOL_NAME`,
    `HELP_COMMAND_NAME`).
*   **Construction:** the factory fills `facets` and builds the struct literally. There is a `new`
    only where something is checked, as in `DefaultToolExecutor::new`.
*   **Parameters:** the turn is always the parameter `turn`.

What components share lives beside the facet: `tool::text_argument` and `id_argument` for tool
parameters, `context::render_sections` for a prompt part that lists entries, and `view::ViewError`,
which a tool passes on with `?`.

## Built-in components

### Remote endpoints

A component that talks to a remote system names it in its `params` as `endpoint`, the same
`remote::RemoteEndpoint` for every one of them (issue 188):

```yaml
endpoint:
  url: https://agent.example
  auth:
    type: bearer
    secret: AGENT_TOKEN
  timeout_secs: 30
```

`auth` names a secret, resolved for whoever acts on every use (see [SECURITY.md](SECURITY.md)).
What `timeout_secs` bounds, and how long a component waits without it, the component says. An
unknown field is refused. In messages the endpoint is shown without credentials, query or fragment
(`RemoteEndpoint::shown`) - also where a library writes it into its own (`redact_in`).

### The components

| Kind | Facets | Params | What it does |
|---|---|---|---|
| `echo_tool` | `ToolProvider` | - | The tool `echo`, for testing. |
| `sleep_tool` | `ToolProvider` | `seconds` (default 5), `requires_approval` (default `false`) | The tool `sleep`, which waits, for testing what happens while a tool runs. |
| `calculator_tool` | `ToolProvider` | - | The tool `calculate`. |
| `stop_tool` | `ToolProvider` | - | The tools `stop`, `bye` and `silence`, with which the agent ends its turn. |
| `create_plan_tool` | `ToolProvider`, `PromptProvider` | - | The tool `create_plan` and a system prompt part telling the model when to plan. |
| `mcp_tool` | `ToolProvider` | `endpoint` (its timeout 10 s, for a call or the discovery of the tools), `requires_approval` (`false`), `classification` (the channel's), `description` (the server's `instructions`) | The tools of a remote MCP server; `classification` is how their results are classified (issue 133), `description` what they are for, as their tool set tells it (issue 194). |
| `notes` | `ToolProvider`, `PromptProvider` | - | Public notes of the agents in a channel: `note`, `revoke_note`, `read_notes` and a prompt part ([MEMORY.md](MEMORY.md)). |
| `private_notes` | `ToolProvider`, `PromptProvider` | - | The agent's private notes: `note_private`, `revoke_private_note` and a prompt part ([MEMORY.md](MEMORY.md)). |
| `agent_memory` | `ToolProvider`, `PromptProvider` | - | What the agent remembers across channels and sessions: `remember`, `forget` and a prompt part ([MEMORY.md](MEMORY.md)). |
| `channel_history` | `ToolProvider` | - | The tool `read_history` for other channels of the session ([MEMORY.md](MEMORY.md)). |
| `static_system_prompt_provider` | `PromptProvider` | `prompt` (required), `position` (`start` or `before_turn_input`, default `start`), `classification` (default public, issue 168) | A fixed prompt part. |
| `channel_members_prompt_provider` | `PromptProvider` | `position` (`start` or `before_turn_input`, default `start`), `changes_only` (default false; for external agents, issue 204) | Tells the agent its own name and handle and who else is in its channel, with their introductions; with `changes_only`, only when that changed, with who joined and who left ([AGENT-TURN.md](AGENT-TURN.md#who-is-in-the-channel)). |
| `tool_sets_prompt_provider` | `PromptProvider` | `position` (`start` or `before_turn_input`, default `start`) | Tells the model which of its tool sets are unavailable and why, nothing while all are available; optional ([TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#tool-sets)). |
| `turn_context_provider` | `ContextProvider` | - | The default context provider of an agent run here: joins the prompt parts by position around the transcript ([AGENT-TURN.md](AGENT-TURN.md)). |
| `external_context_provider` | `ContextProvider` | - | The default context provider of an external agent (issue 214): each prompt part on its own, with the component it came from, for its actor to give the remote agent when it changed ([CHANNELS.md](CHANNELS.md)). |
| `a2a_agent` | `ExternalAgent` | `endpoint` (its `url` the agent's base URL, or its card's own; its timeout 30 s, per request, a streamed one until its stream begins), `classification` (the channel's), `sender_format` (`prefix`) and `sender_style` (`handle`) - how it is told who said what, in the content: A2A has no sender ([AGENT-TURN.md](AGENT-TURN.md#who-said-what)) | An external agent reached over A2A 1.x by JSON-RPC (issue 179): reads its agent card, streams over server-sent events where the card offers it; where not, is answered at once with the task and asks for it until it ends or asks back (issue 186). A card it cannot read, or one that offers no JSON-RPC of A2A 1.x, fails when the agent is asked, not when it is built. |
| `ollama_client` | `LLMClient` | `endpoint` (`url: http://127.0.0.1:11434`, no timeout, no `auth`), `model` (`gemma4:26b`), `think`, `reasoning_history` (`current_turn`), `system_messages` (`only_at_start`), `sender_format` (`prefix`), `sender_style` (`handle`), `context_size` (Ollama's default) | The default model: a model served by Ollama ([CONFIGURATION.md](CONFIGURATION.md#the-model)). Defuses the model's control tokens in every text it sends (issue 149). |
| `default_tool_executor` | `ToolExecutor` | - | The default executor: runs a call on the `ToolProvider` that has the tool ([TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md)). |
| `policy_moderator` | `Moderator` | `max_agent_hops` (`3`) | The default moderator of a channel: routes as every channel does and asks its `ResponderPolicy` whom to ask; ends a chain between agents after `max_agent_hops` hops ([CHANNELS.md](CHANNELS.md#chains-between-agents)). |
| `broadcast_policy` | `ResponderPolicy` | - | Asks every agent. |
| `round_robin_policy` | `ResponderPolicy` | - | The default policy: the agents take turns. |
| `random_policy` | `ResponderPolicy` | - | Asks one agent, chosen at random. |
| `mentions_only_policy` | `ResponderPolicy` | - | Asks nobody: agents answer only when mentioned. |
| `help_command` | `CommandProvider` | - | The user command `/help`. |
| `wakeup_command` | `CommandProvider` | - | The user command `/wakeup`. |

The tools and commands themselves are listed with their parameters in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#built-in-tools-and-commands).
