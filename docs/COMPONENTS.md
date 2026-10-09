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
    failure of the component is skipped or fails the turn. A factory reads `params` with
    `config_of_params` into a configuration type of its own; left out, they count as `{}`, so a
    configuration whose fields all have a default needs none.
*   **`ComponentRegistry`** holds a factory per kind, together with the facets that kind provides.
    `ComponentRegistry::built_in` registers every built-in kind; the binaries hand it to the
    `RuntimeManager` as it is, which applies `component_defaults`. A test registers kinds of its own
    in it first - a stand-in for the model, say (issue 130).
*   **`ComponentContainer`** builds the components of a `ComponentConfig` and hands out their
    facets.

A descriptor of the reserved kind `ref` (`PRECONFIGURED_REF_KIND`) builds no component of its own:
its `params` are `{ "id": "..." }`, naming preconfigured components stored in the `ComponentStore`:
a list of descriptors under one id. The `ref` stands for all of them, in their order, where it is
written. It is read afresh every time it is built, in the context of the graph that refers to it,
so secrets are resolved as that caller and a list the caller may not read cannot be built; the
price is that the components are not shared between references (a preconfigured `mcp_tool`
discovers its tools again for every personality that uses it).

### Sharing components between personalities

What many personalities should have alike - the channel's context, the tools every agent needs - is
kept once, as preconfigured components, and each personality includes it with a `ref` (issue 270):

```yaml
components:                      # of the seed: kept in the ComponentStore
- id: channel-context
  components:
  - { id: channel-members, kind: channel_members_prompt_provider }
  - { id: channel-language, kind: channel_language_prompt_provider }
personalities:
- name: jonathan-assistant
  components:
  - { id: system-prompt, kind: static_system_prompt_provider, params: { ... } }
  - { id: channel-context, kind: ref, params: { id: channel-context } }
  - { id: memory, kind: agent_memory }
```

*   An addition to a list reaches every personality that includes it, the next time its agents are
    built - not a running agent, as a change of its own personality does not either.
*   The entries of a `ComponentConfig` have unique ids, and so have the components they stand for.
    A component of an included list that is written again - beside the `ref`, or through a second
    `ref` - is a configuration error naming the `ref`, not an override; a personality that wants a
    component otherwise does not include the list (or includes a finer one).
*   Preconfigured components cannot include a `ref` themselves: there is no depth and no cycle.
*   A list that cannot be read - gone, or a share withdrawn - fails as `ComponentError::Unavailable`
    (`is_unavailable` also through the component it was building). An agent whose personality
    includes it is left out of its channel, as one whose personality was withdrawn is; the channel
    runs with everyone else ([CHANNELS.md](CHANNELS.md)).
*   What an agent was built from is recorded, not computed again: `ComponentContainer::components`
    gives the configuration with every `ref` resolved, its actor keeps it (`Actor::components`), and
    `MemberView::components` reads it for the agent's owner (`ReadAgentComponents`), as
    `GET …/actors/{aid}/components` does ([REST-SERVER.md](REST-SERVER.md)).
*   Only components are shared. A personality's kind, roles, introduction and reasoning visibility
    are its own.
*   It serves every `ComponentConfig` alike - a channel's components and a session's commands, too.

## Facets

A component offers one or more *facets*: traits it implements to take part in the agent loop.

| Facet | What it contributes | Where the trait lives |
|---|---|---|
| `ToolProvider` | tools the model may call | `crate::tool` |
| `ChannelCommandProvider` | user commands run on the channel | `crate::command` |
| `AgentCommandProvider` | user commands an agent carries out itself, of agents and external agents alike (issue 269) | `crate::command` |
| `PromptProvider` | prompt parts, of agents and external agents alike (issue 204) | `crate::context` |
| `ContextProvider` | what the model is given | `crate::context` |
| `LLMClient` | the language model the agent talks to | `crate::llm` |
| `ExternalAgent` | the remote agent an external personality's agents are (issue 179) | `crate::external` |
| `PerceiveInProcessor` | filtering of incoming messages, of agents and external agents alike | `crate::message` |
| `EmitOutProcessor` | filtering of the messages an agent emits | `crate::message` |
| `Moderator` | how a channel routes its events and who is asked to respond | `crate::moderator` |
| `ResponderPolicy` | whom the `policy_moderator` asks when nobody was mentioned | `crate::moderator` |

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

Of some facets an agent has exactly one: the `ContextProvider` (issue 98) and the `LLMClient` (issue
130), an external agent exactly one `ExternalAgent` (issue 179); a channel has exactly one
`Moderator` and one `ResponderPolicy` (issue 174). They are registered with
`ComponentRegistry::single::<T>(default_kind)` in the `register_all` of the module that holds the
default kind (`component_impl`, `llm`, `external`, `moderator`).

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
    to preconfigured components, which might provide it.
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

*   the `SecurityContext` the graph is built in - for an agent's components, the agent's own, with
    its owner's rights; for a channel's, its owner's. What a component reads while building - an MCP
    server's tools, with its credential - it reads in it. What it asks while a turn runs, it asks in
    the turn's context (`Turn::security_context`, `AgentWork::security_context`): one context per
    piece of work (issue 255),
*   the `SecurityManager`, for the questions a component asks (`AccessQuestions`; the memory asks
    what it may recall, issue 131),
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
after the kind (`calculator_tool.rs`, `notes.rs`, ...), in the module of the facet that is their
purpose. That module's `mod.rs` holds the facet trait - with what consumes it, such as the
`ToolExecutor` - and `register_all`; its other files are the components:

| Module | Components |
|---|---|
| `tool/` | tool providers (`calculator_tool`, `mcp_tool`, ...) |
| `command/` | command providers (`help_command`, `wakeup_command`, `nudge_command`) |
| `llm/` | language models (`ollama_client`) |
| `moderator/` | the moderator and its responder policies |
| `external/` | the ways to reach an external agent (`a2a_agent`, issue 179) - a module per kind: `a2a_agent/` holds the component (`mod.rs`), the protocol's types (`wire.rs`), their translation to and from `crate::external` (`translation.rs`) and the transport (`client.rs`, issue 210) |
| `component_impl/` | everything else, and every component that offers several facets |

A component that offers several facets - `create_plan_tool`, `notes` and the other memory components
provide tools and a prompt part - stays in `component_impl`, because it cannot be sorted by one
facet. Each file holds the kind constant, its `register` function and its tests; each module's
`register_all` registers its kinds and the singles it holds the default for, and
`ComponentRegistry::built_in` calls them all. A new component gets a file of its own and a line in
its module's `register_all`; `component_impl` holds components only.

They follow one convention (issue 101):

*   **Kind constant:** the kind in capitals with `_KIND` (`CALCULATOR_TOOL_KIND`). Only the command
    kinds live in the model, because `AppConfig`'s default names them.
*   **Struct:** the kind in CamelCase, with `Provider` for a `_tool` or `_command` kind
    (`CalculatorToolProvider`) and `Component` for a feature (`NotesComponent`,
    `ChannelHistoryComponent`).
*   **Names:** every tool and command name is a constant (`CALCULATE_TOOL_NAME`,
    `HELP_COMMAND_NAME`).
*   **Construction:** the factory fills `facets` and builds the struct literally. There is a `new`
    only where something is checked.
*   **Parameters:** the turn is always the parameter `turn`.

What components share lives beside the facet: `tool::text_argument` and `id_argument` for tool
parameters, `context::render_sections` for a prompt part that lists entries, and `view::ViewError`,
which a tool passes on with `?`.

## Built-in components

### Remote endpoints

A component that talks to a remote system names it in its `params` as `endpoint`, the same
`caelumora_model::remote::RemoteEndpoint` for every one of them (issue 188) - and so does a storage
of workspaces reached over the network (`workspaces` in [CONFIGURATION.md](CONFIGURATION.md)):

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
| `ask_user_tool` | `ToolProvider` | - | The tool `ask_user`, with which the agent asks users of its channel and waits for their answers (issue 165). |
| `create_plan_tool` | `ToolProvider`, `PromptProvider` | - | The tool `create_plan` and a system prompt part telling the model when to plan. |
| `mcp_tool` | `ToolProvider` | `endpoint` (its timeout 10 s, for a call or the discovery of the tools), `requires_approval` (`true`; `false`, or `"unless_read_only"` for all but the tools the server marks read-only), `classification` (the channel's), `description` (the server's `instructions`), `destination` (`public`) | The tools of a remote MCP server; `classification` is how their results are classified (issue 133), `description` what they are for, as their tool set tells it (issue 194), `destination` what the server is cleared for - its tools are not offered in a channel that may not flow there (issue 180). |
| `notes` | `ToolProvider`, `PromptProvider` | - | Public notes of the agents in a channel: `note`, `revoke_note`, `read_notes` and a prompt part ([MEMORY.md](MEMORY.md)). |
| `private_notes` | `ToolProvider`, `PromptProvider` | - | The agent's private notes: `note_private`, `revoke_private_note` and a prompt part ([MEMORY.md](MEMORY.md)). |
| `agent_memory` | `ToolProvider`, `PromptProvider` | - | What the agent remembers across channels and sessions: `remember`, `forget` and a prompt part ([MEMORY.md](MEMORY.md)). |
| `skills` | `PromptProvider`, `ToolProvider`, `AgentCommandProvider` | `position` (`start` or `before_turn_input`, default `start`), `requires_approval` (`true`, the default, or `false`, for writing) | Skills (issue 22): the catalogue of those the agent may load, `list_skills`, `load_skill`, `create_skill`, `update_skill`, and the commands `/skill` and `/skills` ([SKILLS.md](SKILLS.md)). |
| `channel_history` | `ToolProvider` | - | The tool `read_history` for other channels of the session ([MEMORY.md](MEMORY.md)). |
| `terminal_tool` | `ToolProvider` | `max_wait_secs` (60, at most 300), `max_output_bytes` (64 KiB, of a command's output or of a file), `requires_approval` (`true`; `false`, or `"unless_read_only"` for all but `list_files`, `read_file` and `grep_files`; `command_output`, `kill_command` and `publish_file` never ask), `description` (a text is built in) | The session's workspace, where the agent's channel may use it: its files - `list_files`, `read_file`, `grep_files`, `write_file`, `replace_in_file`, `move_file`, `delete_file`, `publish_file` - and a shell in it - `run_command`, `command_output`, `kill_command` (issue 251, [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#the-workspace)). Generic over the storage: what it can do is what the workspace's terminal can. |
| `artifact_tool` | `ToolProvider` | `requires_approval` (`true`; `false` - `"unless_read_only"` asks too, since publishing changes what is kept), `description` (a text is built in) | Artifacts (issue 271, [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#artifacts)): `publish_artifact`, a file of the session's workspace published as a version of an artifact, kept beyond the workspace. A personality includes it where artifacts are wanted; without an artifact storage configured, its calls fail and say so. |
| `static_system_prompt_provider` | `PromptProvider` | `prompt` (required), `title` (none by default, issue 249), `position` (`start` or `before_turn_input`, default `start`), `classification` (default public, issue 168) | A fixed prompt part, under `[title]` if one is named. |
| `channel_briefing_prompt_provider` | `PromptProvider` | `position` (`start` or `before_turn_input`, default `start`), `changes_only` (default false; for external agents) | Tells the agent what its session and channel are for - their summaries and briefings, as their owners wrote them; nothing if neither says anything ([AGENT-TURN.md](AGENT-TURN.md#what-the-session-and-channel-are-for), issue 245). |
| `channel_language_prompt_provider` | `PromptProvider` | `position` (`start` or `before_turn_input`, default `start`), `changes_only` (default false; for external agents) | Tells the agent the language its channel's conversation is held in, by its names in English and in itself and its tag; nothing if none ([AGENT-TURN.md](AGENT-TURN.md#the-channels-language), issue 56). |
| `channel_members_prompt_provider` | `PromptProvider` | `position` (`start` or `before_turn_input`, default `start`), `changes_only` (default false; for external agents, issue 204) | Tells the agent its own name and handle and who else is in its channel, with their introductions; with `changes_only`, only when that changed, with who joined and who left ([AGENT-TURN.md](AGENT-TURN.md#who-is-in-the-channel)). |
| `workspace_prompt_provider` | `PromptProvider` | `position` (`start` or `before_turn_input`, default `start`), `instruction_files` (file names at the workspace's root, default `["AGENTS.md", "CLAUDE.md"]`) | Tells the agent the workspace it may use in its channel - its name, classification, description and whether it has a shell, and which of the instruction files lie at its root, to be read first (issue 279) - or that there is none; classified as the workspace (issue 251). |
| `tool_sets_prompt_provider` | `PromptProvider` | `position` (`start` or `before_turn_input`, default `start`) | Tells the model which of its tool sets are unavailable and why, nothing while all are available; optional ([TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#tool-sets)). |
| `turn_context_provider` | `ContextProvider` | - | The default context provider of an agent run here: joins the prompt parts by position around the transcript ([AGENT-TURN.md](AGENT-TURN.md)). |
| `external_context_provider` | `ContextProvider` | - | The default context provider of an external agent (issue 214): each prompt part on its own, with the component it came from, for its actor to give the remote agent when it changed ([CHANNELS.md](CHANNELS.md)). |
| `a2a_agent` | `ExternalAgent` | `endpoint` (its `url` the agent's base URL, or its card's own; its timeout 30 s, per request, a streamed one until its stream begins), `classification` (the channel's), `sender_format` (`prefix`) and `sender_style` (`handle`) - how it is told who said what, in the content: A2A has no sender ([AGENT-TURN.md](AGENT-TURN.md#who-said-what)) | An external agent reached over A2A 1.x by JSON-RPC (issue 179): reads its agent card, streams over server-sent events where the card offers it; where not, is answered at once with the task and asks for it until it ends or asks back (issue 186). A card it cannot read, or one that offers no JSON-RPC of A2A 1.x, fails when the agent is asked, not when it is built. |
| `ollama_client` | `LLMClient` | `endpoint` (`url: http://127.0.0.1:11434`, no timeout, no `auth`), `model` (`gemma4:26b`), `think`, `reasoning_history` (`current_turn`), `system_messages` (`only_at_start`), `sender_format` (`prefix`), `sender_style` (`handle`), `context_size` (Ollama's default) | The default model: a model served by Ollama ([CONFIGURATION.md](CONFIGURATION.md#the-model)). Defuses the model's control tokens in every text it sends (issue 149). |
| `policy_moderator` | `Moderator` | `max_agent_hops` (`3`) | The default moderator of a channel: routes as every channel does and asks its `ResponderPolicy` whom to ask; ends a chain between agents after `max_agent_hops` hops ([CHANNELS.md](CHANNELS.md#chains-between-agents)). |
| `broadcast_policy` | `ResponderPolicy` | - | Asks every agent. |
| `round_robin_policy` | `ResponderPolicy` | - | The default policy: the agents take turns. |
| `random_policy` | `ResponderPolicy` | - | Asks one agent, chosen at random. |
| `mentions_only_policy` | `ResponderPolicy` | - | Asks nobody: agents answer only when mentioned. |
| `help_command` | `ChannelCommandProvider` | - | The user command `/help`. |
| `wakeup_command` | `ChannelCommandProvider` | - | The user command `/wakeup`. |
| `nudge_command` | `ChannelCommandProvider` | - | The user command `/nudge`. |

The tools and commands themselves are listed with their parameters in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#built-in-tools-and-commands).
