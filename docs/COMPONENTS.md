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
*   **`ComponentRegistry`** holds a factory per kind, together with the facets that kind provides
    and those of others it needs.
    The binaries hand the built-in registry to the `RuntimeManager`, which applies
    `component_defaults`; a test registers kinds of its own in it first
    (`caelumora_runtime::testing::test_registry`; the agents' tests
    `caelumora_agents::testing::test_registry`, with a model that is not to be asked as the
    default).
*   **`ComponentContainer`** builds the components of a `ComponentConfig` for an owner - an agent,
    an external agent, a channel, a session's commands - and hands out the facets that owner
    declared it asks for, and no other ([What a container accepts](#what-a-container-accepts)).

A descriptor of the reserved kind `ref` builds no component of its own: its `params` name, by `id`,
a list of preconfigured components kept in the `ComponentStore`, and it stands for all of them, in
their order, where it is written. It is read afresh every time it is built, in the context of the
graph that refers to it, so secrets are resolved as that caller and a list the caller may not read
cannot be built. The price is that the components are not shared between references: a
preconfigured `mcp_tool` discovers its tools on its own for every personality that uses it.

### Sharing components between personalities

What many personalities should have alike - the channel's context, the tools every agent needs - is
kept once, as preconfigured components, and each personality includes it with a `ref`:

```yaml
components:                      # of the seed: kept in the ComponentStore
- id: channel-context
  components:
  - { id: channel-participants, kind: channel_participants_prompt_provider }
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
*   A `ref` is read in the name the graph is built in, and also in the name of whoever names it,
    where that is someone else: a channel's components are built in its owner's name, so whoever
    creates the channel may name only lists they may read themselves
    (`ComponentRegistry::require_acceptable`, issue 321).
*   A kind whose params are trusted - where it sends what it is given, what that is cleared for -
    is declared `operator_only` (issue 403): it is written only where an operator writes, in the
    seed's templates, personalities and preconfigured components. A request that writes one itself
    is refused (`ComponentError::OperatorOnly`); it names one through a `ref`.

## Facets

A component offers one or more *facets*: traits it implements to take part in the agent loop. Each
facet trait lives next to the code that uses it, not among the implementations.

| Facet | What it contributes | Where the trait lives |
|---|---|---|
| `ToolProvider` | tools the model may call | `crate::tool` |
| `ChannelCommandProvider` | user commands run on the channel | `crate::command` |
| `AgentCommandProvider` | user commands an agent carries out itself, of agents and external agents alike | `caelumora_agents::command` |
| `PromptProvider` | prompt parts, of agents and external agents alike | `crate::prompt` |
| `ContextProvider` | what the model is given | `crate::prompt` |
| `LLMClient` | the language model the agent talks to | `crate::llm` |
| `ExternalAgent` | the remote agent an external personality's agents are | `crate::external` |
| `PerceiveInProcessor` | filtering of incoming messages, of agents and external agents alike | `crate::message` |
| `EmitOutProcessor` | filtering of the messages an agent emits | `crate::message` |
| `Moderator` | whom a channel's events go to, who is asked to respond, what it tells of its own | `crate::moderator` |
| `ResponderPolicy` | whom the `policy_moderator` asks when nobody was mentioned | `crate::moderator` |
| `CompactionPolicy` | when an agent compacts its context on its own | `crate::compaction` |
| `CompactionStrategy` | how an agent's context is compacted | `crate::compaction` |
| `KnowledgeRestorer` | what a component gave the agent - a skill loaded -, given again after a compaction | `crate::compaction` |

`PerceiveInProcessor` and `EmitOutProcessor` have no built-in implementation; they are extension
points. A processor returns what is to be processed - it may drop, change or add messages - and
records nothing: what the actor was offered is in its channel's history, and what passes in its
transcript.

## Wiring

Components are wired the way Spring Boot wires beans.

*   **A kind declares what it provides** (`ComponentKind::provides`). The components providing a
    facet are thus known before any of them is built, and the container checks that each factory
    kept to what its kind declared.
*   **A kind declares the services it uses** (`ComponentKind::uses`). Its factory is given those
    services of the runtime and no other, and keeps them as fields; one it does not declare is
    refused, naming it.
*   **A kind declares the facets of others it needs** (`ComponentKind::needs`), as the
    `policy_moderator` needs a `ResponderPolicy`. Its factory may ask for those and no other; one
    it does not declare is refused, naming it (`ComponentError::Undeclared`).
*   **A component asks for what it needs, by type** (`context.all::<dyn PromptProvider>()`, or
    `one`). This builds the components that provide the facet on demand, recursively, and returns
    every *other* one in the order of the configuration.
*   **What is asked for is kept as it comes.** Whoever holds a component keeps the `ComponentImpl`
    that `one` or `all` returned, not just its facet. It dereferences to the facet, and its id and
    fail policy are at hand when a call fails.
*   **Nothing is wired by hand.** There are no `deps`: `needs` names facet types, as `provides`
    does, never components, and the container finds who provides them. A `needs` that is forgotten
    is refused the first time the factory asks, naming the facet and the kind, as a forgotten
    `uses` is.
*   **Cycles are refused.** If a component still being built provides what is asked for, that is a
    real cycle, refused naming both components.
*   **The order of the configuration counts.** The components no other one asks for are built in
    that order, and handed out in it.

`uses` and `needs` are two declarations, built alike: a service comes from the runtime, is the same
for every graph and is always there; a facet comes from the container's own graph, is built on
demand and may be missing. Only `needs` decides what a container accepts.

### What a container accepts

A component is built in its owner's name and reads what it reads with its owner's rights - a
secret, a connection opened. What a configuration can set off follows from what its owner does with
it (issue 337):

*   **The owner declares the facets it asks for**, as a constant it keeps, and builds its container
    with them (`ComponentContainer::from_config`). The container refuses `one` and `get_all` of any
    other facet, so that the declaration stays true.
*   **The container accepts their closure**: the components that provide one of those facets - and,
    for a facet of which there is one that no component provides, its default -, then those that
    provide a facet one of these needs, in turn, until nothing is added. It is taken over the
    configuration, not over the registry: a channel accepts what its moderator needs, not what any
    moderator kind might.
*   **A component that provides no facet of the closure is refused before anything is built**,
    named by the entry that brings it - a `ref` too -, so that no secret is read and no connection
    opened for it: a tool in a channel, a moderator in an agent.
*   **Whoever writes a configuration is refused before the verb.** What a container would accept is
    known without building anything (`ComponentRegistry::require_acceptable`, which also reads the
    `ref`s in the writer's name): a channel is refused a tool before it is created
    (`SessionError::InvalidComponents`).
*   **A kind is accepted if one of its facets is in the closure.** `notes` provides a tool and a
    prompt part; an external agent asks for the second alone, and the first is built but never
    called. What a kind does as it is built, it does for all its facets: a facet that reads secrets
    or connects belongs to a kind of its own.
*   **How many does not count.** The closure says which facets are accepted, not how many
    components provide them; that a facet of which there is one has at most one stays as it is.

| Owner | Asks for | Declared as |
|---|---|---|
| agent | `ContextProvider`, `LLMClient`, `CompactionPolicy`, `CompactionStrategy`, `PerceiveInProcessor`, `EmitOutProcessor`, `ToolProvider`, `AgentCommandProvider` | `AgentActor::COMPONENT_FACETS`, registered for its kind (`AgentKinds::facets`) |
| external agent | `ExternalAgent`, `PerceiveInProcessor`, `ContextProvider`, `AgentCommandProvider` | `ExternalAgentActor::COMPONENT_FACETS`, registered for its kind (`AgentKinds::facets`) |
| channel | `Moderator` | `ChannelLoop::COMPONENT_FACETS` |
| a session's commands | `ChannelCommandProvider` | `RuntimeManager::new_command_executor` |

An agent's closure thus holds the `PromptProvider`s its default `turn_context_provider` needs, and
the `LLMClient` and `KnowledgeRestorer`s its default `summary_compaction` needs - which the agent
asks itself, to compact on its own (issue 284), and `compaction` asks for `/compact`.

### Facets of which there is exactly one

Of some facets an agent has exactly one: the `ContextProvider`, the `LLMClient`, the
`CompactionPolicy` and the `CompactionStrategy`; an external agent exactly one `ExternalAgent`; a
channel exactly one `Moderator` and one `ResponderPolicy`. They are registered with
`ComponentRegistry::single` by the module that holds the default kind - or, where the core holds no
kind, by the module of the facet, without one: the `LLMClient`, whose kinds are plugins (issue 329).
The program names their default among the kinds its plugins brought
(`ComponentRegistry::set_default_kind`, refused for a kind that is not registered as providing the
facet): `caelumora-app` and `caelumora-tui` name `ollama_client` of `caelumora-ollama`.

*   Two components of one container that provide such a facet are refused before anything is built.
*   If none does, a default is built the first time the facet is asked for. A container that never
    asks - a session's commands, say - never builds one.
*   A single may have no default where none makes sense: the remote agent of an `ExternalAgent` is
    always named. A container asked for it that provides none fails then; nothing is checked
    beforehand. The `LLMClient` has no default in the core, only the one the program names.
*   What a container's single is, is known without building it (`ComponentRegistry::kind_of`) -
    unless the container refers to preconfigured components, which might provide it.
*   The default comes from, in this order: the container's own component; the defaults the
    container was built with (an external agent's actor gives its `ContextProvider` so,
    `external_context_provider`); the configuration (`AppConfig::component_defaults`, see
    [CONFIGURATION.md](CONFIGURATION.md)); and finally the default kind - the program's
    (`set_default_kind`), or else the one it was registered with. The program's choice is kept
    apart from the declaration: a plugin that declares the single after the program named its
    default - the agents' plugin in a test's registry (issue 370) - does not undo it.

### Factories and tags

*   **Factories may wait.** `register_async` takes a factory that returns a future - for a remote
    system, or for other components; `register` takes one that does not.
*   **Tags** are kept on components, and `get_all` can filter by one, although nothing reads them
    yet.

## What a component may reach

A container is built with a `ComponentContext`, which holds:

*   the `SecurityContext` the graph is built in, the name of its owner - for an agent's components
    the agent's own, with its owner's rights; for a channel's components and a session's commands,
    the owner's, with their roles as they are when it is built (`ControllerManager::owner_context`).
    What a component reads while building - a `ref` resolved, an MCP server's tools, with its
    credential - it reads in it. It does not keep it: what it asks later it asks in the context of
    the act it serves - a tool in its turn's, a moderator in the event's it routes -, one context
    per piece of work;
*   the services of the runtime its kind declares it uses - the `AccessPolicy` for the questions
    a component asks (`AccessQuestions`), the `SecretManager`, the `WorkspaceManager`, the
    manager a plugin provides (the `ArtifactManager`, say) - by type, not as fields of the context,
    so that a new service needs no new field and no component is given one it does not declare. They
    are the runtime's `Services`, which the views and a plugin's routes reach as well;
*   nothing of the session: what a component reads of it - other channels, their notes and
    histories, what its agent may recall - it reads through the `Turn` it is called with (see
    [MEMORY.md](MEMORY.md#reading-other-channels));
*   and, internally, the graph being built, through which each component reaches the others -
    those that provide a facet its kind needs.

Components are not given their actor's `ActorRuntime`: through it they could store the actor's state
or act as the actor past the `Turn` they are called with. What a component may use is exactly what
its context holds, and a service it needs is declared by its kind, which is where it shows who may
use what. A service that acts in someone's name takes the security context with each call; while a
turn runs, a component calls it with the turn's, never with the context it was built in.

## Writing a component

The built-in components live **one file per component kind**, named after the kind
(`calculator_tool.rs`, `notes.rs`, ...), in the module of the facet that is their purpose. Those of
`command/` and `moderator/` are the runtime's own, in `caelumora-runtime/src/`
(`ComponentRegistry::built_in`); the others are an agent's, in `caelumora-agents/src/`, registered
by the agents' plugin (`caelumora_agents::plugin::register_components`, issue 370). That module's
`mod.rs` holds the facet trait - with what consumes it, such as the `ToolExecutor` - and
`register_all`; its other files are the components:

| Module | Components |
|---|---|
| `tool/` | tool providers (`calculator_tool`, `ask_user_tool`, ...) |
| `command/` | command providers (`help_command`, `wakeup_command`, `nudge_command`) |
| `prompt/` | prompt providers (`static_system_prompt_provider`, `channel_participants_prompt_provider`, ...) and the context providers (`turn_context_provider`, `external_context_provider`), with what their parts share (`Prompt`, `Section`, `titled`) and the agents' texts in the prompt catalogue (`PROMPTS`) |
| `llm/` | the facet through which an agent asks its model, the defusing of control tokens any kind may use, and no kind: each model provider is a plugin (`ollama_client` in `caelumora-ollama`) |
| `moderator/` | the moderator and its responder policies |
| `compaction/` | `/compact` (`compaction`, in `command.rs`, since a module cannot hold a file of its own name) and the compaction policies (`threshold_compaction_policy`) and strategies (`summary_compaction`) |
| `external/` | the facet through which an external agent is reached, and no kind: each protocol is a plugin (`a2a_agent` in `caelumora-a2a`) |
| `components/` | everything else, and every component that offers several facets |

A component that offers several facets - `create_plan_tool`, `notes` and the other memory components
provide tools and a prompt part - stays in `components`, because it cannot be sorted by one
facet. Each file holds the kind constant, its `register` function and its tests; each module's
`register_all` registers its kinds and the singles it holds the default for. A new component gets a
file of its own and a line in its module's `register_all`; `components` holds components only.

They follow one convention:

*   **Kind constant:** the kind in capitals with `_KIND` (`CALCULATOR_TOOL_KIND`). Only the command
    kinds live in the model, because `AppConfig`'s default names them.
*   **Struct:** the kind in CamelCase, with `Provider` for a `_tool` or `_command` kind
    (`CalculatorToolProvider`) and `Component` for a feature (`NotesComponent`).
*   **Names:** every tool and command name is a constant (`CALCULATE_TOOL_NAME`).
*   **Texts:** what a prompt part says is a text of the prompt catalogue, its key a constant
    (`TEXT`), rendered from the catalogue the component takes as a service
    ([AGENT-TURN.md](AGENT-TURN.md#the-prompt-catalogue)); so are the descriptions of its tools,
    rendered when it is made (`new`, which fails if one is missing) and kept with its tools
    ([TOOLS.md](TOOLS.md)).
*   **Construction:** the factory builds the struct literally; there is a `new` only where
    something is checked.
*   **Parameters:** the turn is always the parameter `turn`.

What components share lives beside the facet: `tool::text_argument` and `id_argument` for tool
parameters, `prompt::render_sections` for a prompt part that lists entries, and `view::ViewError`,
which a tool passes on with `?`.

## Plugins

A component kind can come from a plugin, a crate of its own that brings a subject the core does not
know. How a plugin registers its kinds, and what of the runtime such a kind may use, is in
[PLUGINS.md](PLUGINS.md).

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
  refresh_secs: 300
```

`auth` names a secret, resolved for whoever acts on every use (see [SECURITY.md](SECURITY.md)). What
`timeout_secs` bounds, and how long a component waits without it, the component says. So does it for
`refresh_secs`: how long what it reads of its remote side - an MCP server's tools, an A2A agent's
card - is kept before it is read again. That is read when it is first needed, in the name of whoever
needs it, never while the component is built, and a read that fails is not kept (`ExpiringCache`,
issue 126). An unknown field is refused. In messages the endpoint is shown without credentials,
query or fragment.

### The components

| Kind | Facets | Params | What it does |
|---|---|---|---|
| `echo_tool` | `ToolProvider` | - | The tool `echo`, for testing. |
| `sleep_tool` | `ToolProvider` | `seconds` (5), `requires_approval` (`false`) | The tool `sleep`, which waits - the model may ask for at most 60 s -, for testing what happens while a tool runs. |
| `calculator_tool` | `ToolProvider` | - | The tool `calculate`. |
| `stop_tool` | `ToolProvider` | - | The tools `stop`, `bye` and `silence`, with which the agent ends its turn: asleep after `stop` and `bye`, idle and without an answer after `silence` ([TOOLS.md](TOOLS.md#built-in-tools)). |
| `ask_user_tool` | `ToolProvider` | - | The tool `ask_user`, with which the agent asks users of its channel and waits for their answers. |
| `create_plan_tool` | `ToolProvider`, `PromptProvider` | - | The tool `create_plan` and a system prompt part telling the model when to plan. |
| `mcp_tool` | `ToolProvider` | `endpoint` (timeout 10 s, per call or discovery; the tools kept for 300 s), `requires_approval` (`true`; `false`, or `"unless_read_only"` for all but the tools the server marks read-only), `classification` (the channel's), `description` (the server's `instructions`), `destination` (`public`) | Of the plugin `caelumora-mcp`, registered where it is joined. The tools of a remote MCP server. `classification` is how their results are classified, `description` what they are for, as their tool set tells it, `destination` what the server is cleared for - its tools are not offered in a channel that may not flow there. |
| `notes` | `ToolProvider`, `PromptProvider` | - | Public notes of the agents in a channel: `write_note`, `update_note`, `revoke_note`, `read_notes`, `reclassify_note` (asks for approval) and a prompt part ([MEMORY.md](MEMORY.md)). |
| `private_notes` | `ToolProvider`, `PromptProvider` | - | The agent's private notes: `write_private_note`, `update_private_note`, `revoke_private_note`, `reclassify_private_note` (asks for approval), all `HIDDEN`, and a prompt part ([MEMORY.md](MEMORY.md)). |
| `agent_memory` | `ToolProvider`, `PromptProvider` | - | What the agent remembers across channels and sessions: `remember`, `update_memory`, `forget`, `reclassify_memory` (asks for approval), all `HIDDEN`, and a prompt part ([MEMORY.md](MEMORY.md)). |
| `skills` | `PromptProvider`, `ToolProvider`, `AgentCommandProvider`, `KnowledgeRestorer` | `position` (`start`), `requires_approval` (`true`, for writing) | Of the plugin `caelumora-skills`, registered where it is joined. Skills: the catalogue of those the agent may load, `list_skills`, `load_skill`, `create_skill`, `update_skill`, and the commands `/skill` and `/skills` ([SKILLS.md](SKILLS.md)). |
| `channel_history` | `ToolProvider` | - | The tool `read_history` for other channels of the session ([MEMORY.md](MEMORY.md)). |
| `terminal_tool` | `ToolProvider` | `max_wait_secs` (60, at most 300), `max_output_bytes` (64 KiB, of a command's output or a file), `requires_approval` (`true`; `false`, or `"unless_read_only"` for all but `list_files`, `read_file` and `grep_files`; `command_output`, `kill_command` and `publish_file` never ask), `description` (built in) | Of the plugin `caelumora-workspace-tools`, registered where it is joined. The session's workspace, where the agent's channel may use it: its files - `list_files`, `read_file`, `grep_files`, `write_file`, `replace_in_file`, `move_file`, `delete_file`, `publish_file` - and a shell in it - `run_command`, `command_output`, `kill_command` ([TOOLS.md](TOOLS.md#the-workspace)). What it can do is what the workspace's terminal can; a turn is offered what its access list lets the agent call there. |
| `artifact_tool` | `ToolProvider` | `requires_approval` (`true`; `false` - `"unless_read_only"` asks too, since publishing changes what is kept), `description` (built in) | Of the plugin `caelumora-artifacts`, registered where it is joined. `publish_artifact`: a file of the session's workspace published as a version of an artifact, kept beyond the workspace ([TOOLS.md](TOOLS.md#artifacts)). Offered where the turn may use a workspace in its channel and an artifact storage is configured. |
| `static_system_prompt_provider` | `PromptProvider` | `prompt` (required), `title` (none), `position` (`start`), `classification` (public) | A fixed prompt part, under `[title]` if one is named. |
| `channel_briefing_prompt_provider` | `PromptProvider` | `position` (`start`), `changes_only` (`false`; for external agents) | Tells the agent what its session and channel are for, as their owners wrote it ([AGENT-TURN.md](AGENT-TURN.md#what-the-session-and-channel-are-for)). |
| `channel_language_prompt_provider` | `PromptProvider` | `position` (`start`), `changes_only` (`false`; for external agents) | Tells the agent the language its channel's conversation is held in ([AGENT-TURN.md](AGENT-TURN.md#the-channels-language)). |
| `current_time_prompt_provider` | `PromptProvider` | `position` (`before_turn_input`) | Tells the agent the current date and time in its session's time zone ([AGENT-TURN.md](AGENT-TURN.md#the-current-time)). |
| `request_reason_prompt_provider` | `PromptProvider` | `position` (`before_turn_input`) | Tells an agent only the responder policy chose that nobody mentioned it, and that with nothing to add it stays silent - by `silence` where it is offered; nothing to an agent asked for another reason ([AGENT-TURN.md](AGENT-TURN.md#why-the-agent-was-asked)). |
| `channel_participants_prompt_provider` | `PromptProvider` | `position` (`start`), `changes_only` (`false`; for external agents) | Tells the agent its own name and handle and who else is in its channel, with their introductions, and when to write an @handle and when a name ([AGENT-TURN.md](AGENT-TURN.md#who-is-in-the-channel)). |
| `channel_mentions_prompt_provider` | `PromptProvider` | `position` (`before_turn_input`) | Reminds the agent, where another agent is in its channel, to write an @handle only for an answer or an action needed now, and a name everywhere else ([AGENT-TURN.md](AGENT-TURN.md#when-to-mention)). |
| `workspace_prompt_provider` | `PromptProvider` | `position` (`start`), `instruction_files` (`["AGENTS.md", "CLAUDE.md"]`) | Of the plugin `caelumora-workspace-tools`, registered where it is joined. Tells the agent the workspace it may use in its channel, and which instruction files lie at its root, to be read first - or that there is none ([AGENT-TURN.md](AGENT-TURN.md#its-workspace)). |
| `tool_sets_prompt_provider` | `PromptProvider` | `position` (`start`) | Tells the model which of its tool sets are unavailable and why, nothing while all are available ([TOOLS.md](TOOLS.md#tool-sets)). |
| `turn_context_provider` | `ContextProvider` | - | The default context provider of an agent run here: joins the prompt parts by position around the transcript ([AGENT-TURN.md](AGENT-TURN.md)). |
| `external_context_provider` | `ContextProvider` | - | The default context provider of an external agent: each prompt part on its own, for its actor to give the remote agent when it changed ([CHANNELS.md](CHANNELS.md)). |
| `a2a_agent` | `ExternalAgent` | `endpoint` (the agent's base URL, or its card's own; timeout 30 s per request, a streamed one until its stream begins; the card kept for 300 s), `classification` (the channel's), `sender_format` (`prefix`), `sender_style` (`handle`), `message_times` (`true`) | Of the plugin `caelumora-a2a`, registered where it is joined. An external agent reached over A2A 1.x by JSON-RPC: streams where its agent card offers it, otherwise asks for the task until it ends or asks back. A card it cannot read, or one without JSON-RPC of A2A 1.x, fails when the agent is asked, not when it is built; an agent not reached where its card said has its card read again at the next request. A2A has no sender, so who said what is told in the content ([AGENT-TURN.md](AGENT-TURN.md#who-said-what)). |
| `ollama_client` | `LLMClient` | `endpoint` (`http://127.0.0.1:11434`, no timeout, no `auth`), `model` (`gemma4:26b`), `think`, `reasoning_history` (`current_turn`), `system_messages` (`only_at_start`), `sender_format` (`prefix`), `sender_style` (`handle`), `message_times` (`true`), `context_size` (Ollama's default) | The default model: a model served by Ollama ([CONFIGURATION.md](CONFIGURATION.md#the-model)). Defuses the model's control tokens in every text it sends. |
| `policy_moderator` | `Moderator` | `mentions_replace_policy` (`anyone`; `agents`), `max_agent_hops` (`3`), `max_span_depth` (`100`), `context_messages` (`10`) | The default moderator of a channel: routes as every channel does, asks the agents a message mentions - nobody for a message to people alone, unless `agents` - and else asks its `ResponderPolicy` whom to ask, told the last `context_messages` messages heard before ([CHANNELS.md](CHANNELS.md#moderators)); ends a chain between agents after `max_agent_hops` hops, and asks nobody to respond to an event `max_span_depth` deep ([CHANNELS.md](CHANNELS.md#chains-between-agents)). |
| `broadcast_policy` | `ResponderPolicy` | - | Asks every agent. |
| `round_robin_policy` | `ResponderPolicy` | - | The default policy: the agents take turns. |
| `random_policy` | `ResponderPolicy` | - | Asks one agent, chosen at random. |
| `mentions_only_policy` | `ResponderPolicy` | - | Asks nobody: agents answer only when mentioned. |
| `user_driven_policy` | `ResponderPolicy` | - | Asks every agent if a user wrote, nobody if an agent did. |
| `decision_policy` | `ResponderPolicy` | `endpoint`, `model` (`nimble`), `keep_alive`, `threshold` (`0.3`), `max_responders` (`1`), `on_failure` (`none`), `destination` (`public`) | Of the plugin `caelumora-decision`, registered where it is joined; configured by an operator alone. Asks a decision model whom to ask ([CHANNELS.md](CHANNELS.md#choosing-by-what-was-said)). |
| `compaction` | `AgentCommandProvider` | - | The command `/compact`, for the agent's owners: asks the agent's `CompactionStrategy` to compact its context and tells the channel what it did ([AGENT-TURN.md](AGENT-TURN.md#compaction)). |
| `threshold_compaction_policy` | `CompactionPolicy` | `threshold` (0.8, how full the context may be, as a share of the model's context size) | The default policy: the agent compacts its context on its own before a call once it is that full, as measured ([AGENT-TURN.md](AGENT-TURN.md#compaction)). Its strategy's `keep` belongs below it, or every compaction frees too little. |
| `summary_compaction` | `CompactionStrategy` | `keep` (0.25, how much of the context is kept as it stands, as a share of the model's context size) | The default strategy: the agent's own model summarises the older part of its transcript, and the agent's `KnowledgeRestorer`s give again what it had been given there ([AGENT-TURN.md](AGENT-TURN.md#compaction)). For agents run here only: it takes the agent's `LLMClient`. |
| `help_command` | `ChannelCommandProvider` | - | The user command `/help` ([COMMANDS.md](COMMANDS.md#built-in-commands)). |
| `wakeup_command` | `ChannelCommandProvider` | - | The user command `/wakeup`. |
| `nudge_command` | `ChannelCommandProvider` | - | The user command `/nudge`. |

`position` is `start` or `before_turn_input`; the default is given in parentheses. Every tool is
`SHOWN` unless its row says otherwise. This table is the one list of what comes with Caelumora; the
commands are described with their arguments and answers in
[COMMANDS.md](COMMANDS.md#built-in-commands).
