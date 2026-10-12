# Components

An agent's behaviour is assembled from components: what tools it has, what its prompt contains, how
its context is built and how its tool calls run. A session's user commands and a channel's moderator
are components too. This document describes how components are declared, registered, wired and
built, and lists the built-in ones. The tools and commands that components contribute are described
in [TOOLS.md](TOOLS.md) and [COMMANDS.md](COMMANDS.md); how the agent uses its components during a
turn is in [AGENT-TURN.md](AGENT-TURN.md).

## Configuration, registry and container

*   **`ComponentConfig`** declares the components of a personality, of a session's commands or of a
    channel (its moderator), as a list of descriptors. Their order matters: it is the order in which
    they are handed out.
*   **`ComponentDescriptor`** configures one component: its `id`, `kind`, `params`, `tags` and
    `fail_policy` - `FailOpen`, the default, skips a failing component, `FailClosed` fails the turn.
    Params left out count as `{}`.
*   **`ComponentRegistry`** holds a factory per kind, together with the facets that kind provides.
    The binaries hand the built-in registry to the `RuntimeManager`, which applies
    `component_defaults`; a test registers kinds of its own in it first.
*   **`ComponentContainer`** builds the components of a `ComponentConfig` and hands out their
    facets.

A descriptor of the reserved kind `ref` builds no component of its own: its `params` name, by `id`,
a list of preconfigured components kept in the `ComponentStore`, and it stands for all of them, in
their order, where it is written. It is read afresh every time it is built, in the context of the
graph that refers to it, so secrets are resolved as that caller and a list the caller may not read
cannot be built. The price is that the components are not shared between references: a
preconfigured `mcp_tool` discovers its tools again for every personality that uses it.

### Sharing components between personalities

What many personalities should have alike - the channel's context, the tools every agent needs - is
kept once, as preconfigured components, and each personality includes it with a `ref`:

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

*   An addition to a list reaches every personality that includes it the next time its agents are
    built - not a running agent, as a change of its own personality does not either.
*   Ids are unique across a `ComponentConfig` and the components its `ref`s stand for. A component
    of an included list written again - beside the `ref` or through a second one - is a
    configuration error naming the `ref`, not an override; a personality that wants a component
    otherwise does not include the list, or includes a finer one.
*   Preconfigured components cannot include a `ref` themselves: there is no depth and no cycle.
*   A list that cannot be read - gone, or a share withdrawn - fails as unavailable. An agent whose
    personality includes it is left out of its channel, as one whose personality was withdrawn is;
    the channel runs with everyone else ([CHANNELS.md](CHANNELS.md)).
*   What an agent was built from is recorded, not computed again: the configuration with every
    `ref` resolved is kept by its actor and shown to the agent's owner (`ReadAgentComponents`).
*   Only components are shared. A personality's kind, roles, introduction and reasoning visibility
    are its own.
*   It serves every `ComponentConfig` alike - a channel's components and a session's commands too.

## Facets

A component offers one or more *facets*: traits it implements to take part in the agent loop. Each
facet trait lives next to the code that uses it, not among the implementations.

| Facet | What it contributes | Where the trait lives |
|---|---|---|
| `ToolProvider` | tools the model may call | `crate::tool` |
| `ChannelCommandProvider` | user commands run on the channel | `crate::command` |
| `AgentCommandProvider` | user commands an agent carries out itself, of agents and external agents alike | `crate::command` |
| `PromptProvider` | prompt parts, of agents and external agents alike | `crate::context` |
| `ContextProvider` | what the model is given | `crate::context` |
| `LLMClient` | the language model the agent talks to | `crate::llm` |
| `ExternalAgent` | the remote agent an external personality's agents are | `crate::external` |
| `PerceiveInProcessor` | filtering of incoming messages, of agents and external agents alike | `crate::message` |
| `EmitOutProcessor` | filtering of the messages an agent emits | `crate::message` |
| `Moderator` | how a channel routes its events and who is asked to respond | `crate::moderator` |
| `ResponderPolicy` | whom the `policy_moderator` asks when nobody was mentioned | `crate::moderator` |
| `CompactionStrategy` | how an agent's context is compacted | `crate::compaction` |
| `KnowledgeRestorer` | what a component gave the agent - a skill loaded -, given again after a compaction | `crate::compaction` |

`PerceiveInProcessor` and `EmitOutProcessor` have no built-in implementation; they are extension
points. A processor returns what is to be processed - it may drop, change or add messages - and
records nothing: the actor keeps all it was offered in its history, and what passes in its
transcript.

## Wiring

Components are wired the way Spring Boot wires beans.

*   **A kind declares what it provides** (`ComponentKind::provides`). The components providing a
    facet are thus known before any of them is built, and the container checks that each factory
    kept to what its kind declared.
*   **A kind declares the services it uses** (`ComponentKind::uses`). Its factory is given those
    services of the runtime and no other, and keeps them as fields; one it does not declare is
    refused, naming it.
*   **A component asks for what it needs, by type** (`context.all::<dyn PromptProvider>()`, or
    `one`). This builds the components that provide the facet on demand, recursively, and returns
    every *other* one in the order of the configuration.
*   **What is asked for is kept as it comes.** Whoever holds a component keeps the `ComponentImpl`
    that `one` or `all` returned, not just its facet. It dereferences to the facet, and its id and
    fail policy are at hand when a call fails.
*   **Nothing is wired by hand.** There are no `deps`, so no dependency can be forgotten.
*   **Cycles are refused.** If a component still being built provides what is asked for, that is a
    real cycle, refused naming both components.
*   **The order of the configuration counts.** The components nobody asks for are built in that
    order, and handed out in it.

### Facets of which there is exactly one

Of some facets an agent has exactly one: the `ContextProvider`, the `LLMClient` and the
`CompactionStrategy`; an external agent exactly one `ExternalAgent`; a channel exactly one
`Moderator` and one `ResponderPolicy`. They are registered with `ComponentRegistry::single` by the
module that holds the default kind.

*   Two components of one container that provide such a facet are refused before anything is built.
*   If none does, a default is built the first time the facet is asked for. A container that never
    asks - a session's commands, say - never builds one.
*   A single may have no default where none makes sense: the remote agent of an `ExternalAgent` is
    always named. A container asked for it that provides none fails then; nothing is checked
    beforehand.
*   What a container's single is, is known without building it (`ComponentRegistry::kind_of`) -
    unless the container refers to preconfigured components, which might provide it.
*   The default comes from, in this order: the container's own component; the defaults the
    container was built with (an external agent's actor gives its `ContextProvider` so,
    `external_context_provider`); the configuration (`AppConfig::component_defaults`, see
    [CONFIGURATION.md](CONFIGURATION.md)); and finally the kind registered in code.

### Factories and tags

*   **Factories may wait.** `register_async` takes a factory that returns a future - for a remote
    system, or for other components; `register` takes one that does not.
*   **Tags** are kept on components, and `get_all` can filter by one, although nothing reads them
    yet.

## What a component may reach

A container is built with a `ComponentContext`, which holds:

*   the `SecurityContext` the graph is built in - for an agent's components the agent's own, with
    its owner's rights; for a channel's, its owner's. What a component reads while building - an MCP
    server's tools, with its credential - it reads in it. What it asks while a turn runs, it asks in
    the turn's context: one context per piece of work;
*   the services of the runtime its kind declares it uses - the `SecurityManager` for the questions
    a component asks (`AccessQuestions`), the `SecretManager`, the `WorkspaceManager`, the
    `ArtifactManager`, the manager a plugin provides - by type, not as fields of the context, so
    that a new service needs no new field and no component is given one it does not declare. They
    are the runtime's `Services`, which the views and a plugin's routes reach as well;
*   nothing of the session: what a component reads of it - other channels, their notes and
    histories, what its agent may recall - it reads through the `Turn` it is called with (see
    [MEMORY.md](MEMORY.md#reading-other-channels));
*   and, internally, the graph being built, through which each component reaches the others.

Components are not given their actor's `ActorRuntime`: through it they could store the actor's state
or act as the actor past the `Turn` they are called with. What a component may use is exactly what
its context holds, and a service it needs is declared by its kind, which is where it shows who may
use what. A service that acts in someone's name takes the security context with each call; while a
turn runs, a component calls it with the turn's, never with the context it was built in.

## Writing a component

The built-in components live in `caelumora-runtime/src/`, **one file per component kind**, named
after the kind (`calculator_tool.rs`, `notes.rs`, ...), in the module of the facet that is their
purpose. That module's `mod.rs` holds the facet trait - with what consumes it, such as the
`ToolExecutor` - and `register_all`; its other files are the components:

| Module | Components |
|---|---|
| `tool/` | tool providers (`calculator_tool`, `terminal_tool`, ...) |
| `command/` | command providers (`help_command`, `wakeup_command`, `nudge_command`) |
| `llm/` | language models (`ollama_client`) |
| `moderator/` | the moderator and its responder policies |
| `compaction/` | `/compact` (`compaction`, in `command.rs`, since a module cannot hold a file of its own name) and the compaction strategies (`summary_compaction`) |
| `external/` | the ways to reach an external agent - a module per kind (`a2a_agent/`) |
| `component_impl/` | everything else, and every component that offers several facets |

A component that offers several facets - `create_plan_tool`, `notes` and the other memory components
provide tools and a prompt part - stays in `component_impl`, because it cannot be sorted by one
facet. Each file holds the kind constant, its `register` function and its tests; each module's
`register_all` registers its kinds and the singles it holds the default for. A new component gets a
file of its own and a line in its module's `register_all`; `component_impl` holds components only.

They follow one convention:

*   **Kind constant:** the kind in capitals with `_KIND` (`CALCULATOR_TOOL_KIND`). Only the command
    kinds live in the model, because `AppConfig`'s default names them.
*   **Struct:** the kind in CamelCase, with `Provider` for a `_tool` or `_command` kind
    (`CalculatorToolProvider`) and `Component` for a feature (`NotesComponent`).
*   **Names:** every tool and command name is a constant (`CALCULATE_TOOL_NAME`).
*   **Construction:** the factory builds the struct literally; there is a `new` only where
    something is checked.
*   **Parameters:** the turn is always the parameter `turn`.

What components share lives beside the facet: `tool::text_argument` and `id_argument` for tool
parameters, `context::render_sections` for a prompt part that lists entries, and `view::ViewError`,
which a tool passes on with `?`.

## Plugins

A plugin (issue 323) is a crate linked into the program that brings a subject the core does not
know - a kind of entity with what keeps it, its components, its views and its routes - or a backend
of a service of the core (issue 324). It implements `caelumora_runtime::plugin::Plugin` - a name for
messages, `backends` and `register` - and the program lists it: `caelumora-app` in `rest_plugins()`
if it serves routes, otherwise in `plugins()`, and builds its runtime with both; `caelumora-tui` in
its own `plugins()`. A program builds its runtime with `RuntimeBuilder::from_config`, given the
configuration and its plugins, in two steps:

1.  Each plugin registers its backends (`backends`, see
    [ARCHITECTURE.md](ARCHITECTURE.md#backends)) with the core's (`Backends::with_plugins`), before
    anything is made: what the core's services are made of has to be known first.
2.  The policies, the stores, the secrets and the built-in component kinds are made, then the
    core's services and managers (`RuntimeBuilder::new`), with the backends the configuration
    names; then each plugin registers the rest of what it brings (`register`), and `build` makes
    the runtime.

A test that builds the runtime from parts of its own calls `RuntimeBuilder::new` with
`Backends::with_plugins` of its plugins and registers them afterwards; `RuntimeManager::new` is the
builder without plugins. A backend's factory is registered with `Backends::register` for the
service it serves (`dyn TerminalProvider`, say), reads its parameters with `config_of_params` and
asks the services it is given - the policies, the `SecretManager`, the store of the users - with
`service_of`. While it registers, a plugin

*   asks for the store of its kind (`entity_store::<T>()`). The configured backend opens a store
    for any kind (`EntityStoreFactory`), once - whoever asks for the kind again is given the same
    store - and refuses a kind whose name another kind has;
*   reaches the core's services it needs for its manager - the event bus, the policies, the
    secrets, the controllers - and offers its manager as a service (`provide`), which its component
    kinds use (`ComponentKind::uses`), its views read through (`Views::service`) and its routes
    take when they are built;
*   registers its component kinds (`components()`), as `register_all` does for the built-in ones;
*   makes its kind go with what holds the scope it lies in (`scope_member`, a `ScopeMember`);
*   claims its section of the configuration (`config::<C>(name)`), read as `C`, and of the seed
    files (`seed_section`, a `SeedSection`). Both sections stand at the top level of their files;
    one that no plugin claims is an error, so that a misspelt section is found.

Its views are an extension trait on `Views`, built from what `Views` opens: the viewer's `context`,
the `security` and `confidentiality` policies, its manager as a `service`, and `channel_reading`.
Its routes come from `caelumora_rest_server::plugin::RestPlugin`, a `Plugin` that builds a `Router`
with the `AppState`; the API serves them behind the same login as the core's (`routes::api`). A
plugin that brings a component kind alone, such as `caelumora-mcp` (issue 325), implements `Plugin`
only and registers the kind in `register`. A plugin's tests use the world of the security suite -
the runtime with the recording ownership policy and three users -, `caelumora_runtime::testing`, as
a dev-dependency with the feature `test-support`.

### What a component kind outside the core may use

A component kind in a crate of its own uses the runtime through what a component of the core uses
too, and nothing else - there is no prelude that re-exports at large, so that it shows which types
a plugin depends on:

*   **To register it:** `ComponentRegistry::register` or `register_async`, `ComponentKind` with the
    facets it `provides` and the services it `uses`, `ComponentFacets`, and `config_of_params` to
    read its `params`.
*   **While it is built:** the `ComponentContext` - `security_context`, in whose name it is built,
    and `service::<T>()`, the services of the runtime it declared.
*   **The facets it offers**, with what their signatures name: for a tool provider `ToolProvider`,
    `ToolOutput`, `ToolError` and `ToolResult`, `ApprovalMode`, and the `Turn` a call is made in.
*   **The model's types** of what it handles: `Tool`, `RemoteEndpoint`, `SecretManager`,
    `Classification`, ...

What a plugin needs beyond that is made visible when a plugin needs it, as part of the runtime's
API, rather than reached around (issue 325).

## Built-in components

### Remote endpoints

A component that talks to a remote system names it in its `params` as `endpoint`, the same
`RemoteEndpoint` for every one of them - and so does a storage of workspaces reached over the
network (`workspaces` in [CONFIGURATION.md](CONFIGURATION.md)):

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
unknown field is refused. In messages the endpoint is shown without credentials, query or fragment.

### The components

| Kind | Facets | Params | What it does |
|---|---|---|---|
| `echo_tool` | `ToolProvider` | - | The tool `echo`, for testing. |
| `sleep_tool` | `ToolProvider` | `seconds` (5), `requires_approval` (`false`) | The tool `sleep`, which waits - the model may ask for at most 60 s -, for testing what happens while a tool runs. |
| `calculator_tool` | `ToolProvider` | - | The tool `calculate`. |
| `stop_tool` | `ToolProvider` | - | The tools `stop`, `bye` and `silence`, with which the agent ends its turn: asleep after `stop` and `bye`, idle and without an answer after `silence` ([TOOLS.md](TOOLS.md#built-in-tools)). |
| `ask_user_tool` | `ToolProvider` | - | The tool `ask_user`, with which the agent asks users of its channel and waits for their answers. |
| `create_plan_tool` | `ToolProvider`, `PromptProvider` | - | The tool `create_plan` and a system prompt part telling the model when to plan. |
| `mcp_tool` | `ToolProvider` | `endpoint` (timeout 10 s, per call or discovery), `requires_approval` (`true`; `false`, or `"unless_read_only"` for all but the tools the server marks read-only), `classification` (the channel's), `description` (the server's `instructions`), `destination` (`public`) | Of the plugin `caelumora-mcp`, registered where it is joined. The tools of a remote MCP server. `classification` is how their results are classified, `description` what they are for, as their tool set tells it, `destination` what the server is cleared for - its tools are not offered in a channel that may not flow there. |
| `notes` | `ToolProvider`, `PromptProvider` | - | Public notes of the agents in a channel: `write_note`, `update_note`, `revoke_note`, `read_notes`, `reclassify_note` (asks for approval) and a prompt part ([MEMORY.md](MEMORY.md)). |
| `private_notes` | `ToolProvider`, `PromptProvider` | - | The agent's private notes: `write_private_note`, `update_private_note`, `revoke_private_note`, `reclassify_private_note` (asks for approval), all `HIDDEN`, and a prompt part ([MEMORY.md](MEMORY.md)). |
| `agent_memory` | `ToolProvider`, `PromptProvider` | - | What the agent remembers across channels and sessions: `remember`, `update_memory`, `forget`, `reclassify_memory` (asks for approval), all `HIDDEN`, and a prompt part ([MEMORY.md](MEMORY.md)). |
| `skills` | `PromptProvider`, `ToolProvider`, `AgentCommandProvider`, `KnowledgeRestorer` | `position` (`start`), `requires_approval` (`true`, for writing) | Of the plugin `caelumora-skills`, registered where it is joined. Skills: the catalogue of those the agent may load, `list_skills`, `load_skill` (`WITHOUT_RESULT`), `create_skill`, `update_skill`, and the commands `/skill` and `/skills` ([SKILLS.md](SKILLS.md)). |
| `channel_history` | `ToolProvider` | - | The tool `read_history` for other channels of the session ([MEMORY.md](MEMORY.md)). |
| `terminal_tool` | `ToolProvider` | `max_wait_secs` (60, at most 300), `max_output_bytes` (64 KiB, of a command's output or a file), `requires_approval` (`true`; `false`, or `"unless_read_only"` for all but `list_files`, `read_file` and `grep_files`; `command_output`, `kill_command` and `publish_file` never ask), `description` (built in) | The session's workspace, where the agent's channel may use it: its files - `list_files`, `read_file`, `grep_files`, `write_file`, `replace_in_file`, `move_file`, `delete_file`, `publish_file` - and a shell in it - `run_command`, `command_output`, `kill_command` ([TOOLS.md](TOOLS.md#the-workspace)). What it can do is what the workspace's terminal can. |
| `artifact_tool` | `ToolProvider` | `requires_approval` (`true`; `false` - `"unless_read_only"` asks too, since publishing changes what is kept), `description` (built in) | `publish_artifact`: a file of the session's workspace published as a version of an artifact, kept beyond the workspace ([TOOLS.md](TOOLS.md#artifacts)). Without an artifact storage configured, its calls fail and say so. |
| `static_system_prompt_provider` | `PromptProvider` | `prompt` (required), `title` (none), `position` (`start`), `classification` (public) | A fixed prompt part, under `[title]` if one is named. |
| `channel_briefing_prompt_provider` | `PromptProvider` | `position` (`start`), `changes_only` (`false`; for external agents) | Tells the agent what its session and channel are for, as their owners wrote it ([AGENT-TURN.md](AGENT-TURN.md#what-the-session-and-channel-are-for)). |
| `channel_language_prompt_provider` | `PromptProvider` | `position` (`start`), `changes_only` (`false`; for external agents) | Tells the agent the language its channel's conversation is held in ([AGENT-TURN.md](AGENT-TURN.md#the-channels-language)). |
| `channel_members_prompt_provider` | `PromptProvider` | `position` (`start`), `changes_only` (`false`; for external agents) | Tells the agent its own name and handle and who else is in its channel, with their introductions ([AGENT-TURN.md](AGENT-TURN.md#who-is-in-the-channel)). |
| `workspace_prompt_provider` | `PromptProvider` | `position` (`start`), `instruction_files` (`["AGENTS.md", "CLAUDE.md"]`) | Tells the agent the workspace it may use in its channel, and which instruction files lie at its root, to be read first - or that there is none ([AGENT-TURN.md](AGENT-TURN.md#its-workspace)). |
| `tool_sets_prompt_provider` | `PromptProvider` | `position` (`start`) | Tells the model which of its tool sets are unavailable and why, nothing while all are available ([TOOLS.md](TOOLS.md#tool-sets)). |
| `turn_context_provider` | `ContextProvider` | - | The default context provider of an agent run here: joins the prompt parts by position around the transcript ([AGENT-TURN.md](AGENT-TURN.md)). |
| `external_context_provider` | `ContextProvider` | - | The default context provider of an external agent: each prompt part on its own, for its actor to give the remote agent when it changed ([CHANNELS.md](CHANNELS.md)). |
| `a2a_agent` | `ExternalAgent` | `endpoint` (the agent's base URL, or its card's own; timeout 30 s per request, a streamed one until its stream begins), `classification` (the channel's), `sender_format` (`prefix`), `sender_style` (`handle`) | An external agent reached over A2A 1.x by JSON-RPC: streams where its agent card offers it, otherwise asks for the task until it ends or asks back. A card it cannot read, or one without JSON-RPC of A2A 1.x, fails when the agent is asked, not when it is built. A2A has no sender, so who said what is told in the content ([AGENT-TURN.md](AGENT-TURN.md#who-said-what)). |
| `ollama_client` | `LLMClient` | `endpoint` (`http://127.0.0.1:11434`, no timeout, no `auth`), `model` (`gemma4:26b`), `think`, `reasoning_history` (`current_turn`), `system_messages` (`only_at_start`), `sender_format` (`prefix`), `sender_style` (`handle`), `context_size` (Ollama's default) | The default model: a model served by Ollama ([CONFIGURATION.md](CONFIGURATION.md#the-model)). Defuses the model's control tokens in every text it sends. |
| `policy_moderator` | `Moderator` | `max_agent_hops` (`3`) | The default moderator of a channel: routes as every channel does and asks its `ResponderPolicy` whom to ask; ends a chain between agents after `max_agent_hops` hops ([CHANNELS.md](CHANNELS.md#chains-between-agents)). |
| `broadcast_policy` | `ResponderPolicy` | - | Asks every agent. |
| `round_robin_policy` | `ResponderPolicy` | - | The default policy: the agents take turns. |
| `random_policy` | `ResponderPolicy` | - | Asks one agent, chosen at random. |
| `mentions_only_policy` | `ResponderPolicy` | - | Asks nobody: agents answer only when mentioned. |
| `compaction` | `AgentCommandProvider` | - | The command `/compact`, for the agent's owners: asks the agent's `CompactionStrategy` to compact its context and tells the channel what it did ([AGENT-TURN.md](AGENT-TURN.md#compaction)). |
| `summary_compaction` | `CompactionStrategy` | `keep_last` (10, the messages kept as they stand) | The default strategy: the agent's own model summarises the older part of its transcript, and the agent's `KnowledgeRestorer`s give again what it had been given there ([AGENT-TURN.md](AGENT-TURN.md#compaction)). For agents run here only: it takes the agent's `LLMClient`. |
| `help_command` | `ChannelCommandProvider` | - | The user command `/help` ([COMMANDS.md](COMMANDS.md#built-in-commands)). |
| `wakeup_command` | `ChannelCommandProvider` | - | The user command `/wakeup`. |
| `nudge_command` | `ChannelCommandProvider` | - | The user command `/nudge`. |

`position` is `start` or `before_turn_input`; the default is given in parentheses. Every tool is
`SHOWN` unless its row says otherwise. This table is the one list of what comes with Caelumora; the
commands are described with their arguments and answers in
[COMMANDS.md](COMMANDS.md#built-in-commands).
