# Caelumora Architecture

Caelumora is a multi-agent system: people and AI agents talk to each other in channels, and agents
act through tools. This document draws the overall picture - what the parts are, how a message
travels through them, and the principles the design follows. Every section ends with the document
that holds the details; the list at the end names them all.

## Crates

The project is a Rust workspace, with a web UI next to it; how its crates depend on each other is
drawn in [CRATES.md](CRATES.md):

*   **`caelumora-domain`** - the domain: entities (`Session`, `Channel`, `Agent`, `Personality`,
    `User`, `SessionTemplate`, `Workspace`, `Group`, `Project`), the values with rules
    their fields hold (`value`: `Name`, `Language`, free text such as `Summary`), messages and
    events, what an entity is to be stored and how a store fails (`store`), what a query of a list
    asks (`query`), what an entity carries of who may reach it (`SecurityAttributes`) and what a
    refusal says (`Refused`), the classifications and the one flow rule over them, and the
    configuration (`AppConfig`). It holds data and the rules over its values, but decides nothing
    in anyone's name and runs nothing.
*   **`caelumora-security`** - who may do what, and what may flow where (issue 517): the
    `SecurityContext` every operation is asked in, the actions asked about (`ActionType`), the
    access policy (`AccessPolicy`, `OwnershipPolicy`), the policy of confidentiality
    (`ConfidentialityPolicy`, `ConfidentialityRules`) and of privacy (`PrivacyPolicy`), the
    `Policies` of a process as the configuration chooses them, and the `SecretManager`. It depends
    on the domain; the stores and the kernel decide by it, and a library does not link it. See
    [SECURITY.md](SECURITY.md).
*   **`caelumora-store`** - the stores (issue 517): the entities of every kind behind one checked
    interface (`EntityStore`), the stores of the states, the histories and the transcripts, opened
    from a backend (`Persistence`, `PersistenceBackend`), the checked store every backend is asked
    through (`CheckedStore`), the store in memory and the suite every backend runs. It depends on
    the domain and `caelumora-security`; the kernel opens it, a plugin of the stores brings a
    backend of it, and a library does not link it. See [STORAGE.md](STORAGE.md).
*   **`caelumora-kernel`** - the kernel, where everything runs: sessions, channels and the actors of
    their users and approvers, the kinds of agent registered with it (`AgentKinds`), components, the
    views, the event bus, the controllers that keep entities live, workspaces, the storages of
    binary content, and the user providers and the backends of the secrets. Its centre is the
    `Kernel`, built once in a process by the `KernelBuilder`; what a channel or an actor runs with
    is its *runtime*, which the kernel gives it - a channel its `ChannelRuntime`, a participant's
    actor its `ParticipantRuntime`, an agent's actor its `ActorRuntime` -, and an actor reaches its
    channel through its runtime alone. What a library is given and declares - the kernel's API - is
    a crate of its own, `caelumora-api`.
*   **`caelumora-api`** - the kernel's API (issue 516): what a library is given and declares - an
    actor and its `ActorRuntime`, the readers, a workspace's terminal, the components as a library
    declares and is given them, the `LibraryBuilder`, the facet of a model and its profile - and the
    errors they answer in, with how they are told to a person. It depends on the domain alone; the
    kernel implements its traits, and a library links it and the domain, never the kernel.
*   **`caelumora-agents`** - what an agent does in a channel (issue 370): both kinds of agent - a
    model run here, or an agent that runs elsewhere -, the agent's cognitive cycle and its turn, and
    the facets its components serve (tools, prompt parts, message stages, its commands, what it does
    of its own accord) with their built-in kinds. It joins the kernel as a library of
    the agent side (see [Principles](#principles)), `AgentsLibrary`, an essential one: without it,
    the program is a chat between users.
*   **`caelumora-rest-api`** - the wire of the REST API: the requests and responses, as `serde`
    reads and writes them, made from the domain's entities and snapshots. It depends on the domain
    alone, so that a client reads the API without the server (issue 323).
*   **`caelumora-rest-server`** - a crate the program links: the REST API and the server-sent event
    streams, with authentication. It maps what the kernel answers to the responses of
    `caelumora-rest-api` and decides nothing itself. Its handlers' building blocks - the logged-in
    user, `ApiError`, the extractors, the shared state - are public, and the routes of the API, the
    plugins' among them, are a `Router` to nest. See [REST-SERVER.md](REST-SERVER.md).
*   **`caelumora-app`** - the program: the composition root that lists the plugins and the
    libraries, builds the kernel from the configuration with them, serves the REST API under `/api`
    and the web UI from the binary (feature `web-ui`, on by default).
*   **`caelumora-skills`** - skills, the first plugin (see [Plugins](#plugins)): the entity, its
    manager, views and routes, and the component of the library beside the plugin, in layers by
    Cargo features (issue 516).
*   **`caelumora-artifacts`** - artifacts, a plugin and a library built as `caelumora-skills` is,
    which keeps their content in the kernel's storages of content (issue 282) and extends the view
    of a channel with what was published in it (issue 328), and the view of a project with what lies
    in it (issue 467). See [TOOLS.md](TOOLS.md#artifacts).
*   **`caelumora-mcp`** - the tools of MCP servers, a library that brings nothing but its component
    kind `mcp_tool`, and with it `rmcp` (issue 325). See [TOOLS.md](TOOLS.md#mcp-servers).
*   **`caelumora-compaction`** - compaction of an agent's context (issue 464), a library that brings
    the component `compaction` - an activity of the agent and `/compact` -, the strategies, with the
    facet `CompactionStrategy` and the `KnowledgeRestorer` others serve, and their texts. See
    [AGENT-TURN.md](AGENT-TURN.md#compaction).
*   **`caelumora-a2a`** - external agents reached over the Agent2Agent protocol, a library that
    brings nothing but its component kind `a2a_agent`, so that the agents keep the facet
    `ExternalAgent` and name no protocol (issue 326). See
    [Agents that run elsewhere](#agents-that-run-elsewhere).
*   **`caelumora-model`** - the language models the platform offers (issue 520), a plugin: the
    section `plugins.model`, the service that builds each agent its client of one - which makes
    every call in the agent's name as the kernel holds it then -, and the kind `language_model`, the
    default of the model's facets. A model is needed only within an agent, so it is no part of the
    kernel. See [CONFIGURATION.md](CONFIGURATION.md#the-model).
*   **`caelumora-model-ollama`** - language models served by Ollama, a plugin that brings nothing
    but the backend `ollama` of the models of `caelumora-model`, so that neither names a model
    provider; the programs offer a local one where the configuration names no model (issues 329,
    520).
*   **`caelumora-decision`** - decision models, which answer typed questions about a text with
    probabilities, reached through the `/v1/systemone` contract: a plugin that brings nothing but
    its component kind `decision_policy`, a `ResponderPolicy` that chooses who answers by what was
    said (issue 403). See [CHANNELS.md](CHANNELS.md#choosing-by-what-was-said).
*   **`caelumora-comfyui`** - images generated by a ComfyUI server (issue 512), a library that
    claims the section `libraries.comfyui` of the configuration, reads the workflows it names,
    checked whole, and brings the component kind `comfyui_tool`, whose tool generates images with
    them. See [TOOLS.md](TOOLS.md#comfyui).
*   **`caelumora-open-terminal`** - the storages of workspaces backed by Open Terminal,
    `open_terminal_shared` and `open_terminal_docker`, a plugin that brings nothing but these
    backends, the Docker one with `bollard` behind the feature `docker` (issue 327). See
    [WORKSPACE.md](WORKSPACE.md#storages).
*   **`caelumora-store-json`** - the stores in JSON files below a directory (`JsonFileStore`), a
    plugin that brings nothing but the backend `json` of the stores (issue 446). See
    [STORAGE.md](STORAGE.md#the-entity-store).
*   **`caelumora-store-postgres`** - the stores in Postgres (`PostgresStore`, with `sqlx`), a plugin
    that brings nothing but the backend `postgres` of the stores, which keeps the entities and the
    states (issues 03, 04). See [STORAGE.md](STORAGE.md#in-postgres).
*   **`caelumora-workspace-tools`** - what an agent is given to work on the session's workspace, a
    library that brings nothing but its component kinds `terminal_tool`, the workspace's files and
    its shell, and `workspace_prompt_provider`, the prompt part that names them (issue 428). See
    [TOOLS.md](TOOLS.md#the-workspace).
*   **`caelumora-web-ui`** - the browser frontend (React, TypeScript), built on the REST API and
    served by `caelumora-app`. See [WEB-UI.md](WEB-UI.md).
*   **`caelumora-tui`** - a terminal frontend that runs the kernel in its own process.
*   **`caelumora-logging`** - the setup of the log the two programs share: it installs the `tracing`
    subscriber from `AppConfig::logging`. The other crates only emit records (issue 330).
*   **`caelumora-rest-client`** - reserved for a client of the REST interface; empty so far.

## Plugins

A subject that is not the kernel's - a kind of entity with its manager, its components, its views
and its routes - can be a **plugin**: a crate linked into the program, not loaded at run time (issue
323). It implements `Plugin` and registers what it brings with the `KernelBuilder` when the kernel
is built: its component kinds, the store of its kind, which the configured backend opens for any
kind, its manager as a service of the kernel, which its views and its routes reach, what an agent's
work does with its kind, bound to the views of that work (`extend_readers`, `extend_writers`, issue
516), the members of a scope, and its sections of the configuration and of the seed. On the REST
side, `RestPlugin` adds its routes, served behind the same login as the kernel's. A plugin's views
are an extension trait on the kernel's `Views`, built from what they open, and what a channel's view
lists of its kind one on `ChannelView`. Which plugins are in is the program's choice:
`caelumora-app` and `caelumora-tui` each list theirs, and name the defaults the kernel leaves open -
the model; the kernel knows none. A plugin is one crate; one that brings a library beside it is cut
into layers by Cargo features rather than crates. The first is `caelumora-skills`
([SKILLS.md](SKILLS.md#where-the-code-lives)), the second `caelumora-artifacts`, which depends on
the kernel's workspaces; `caelumora-decision` brings a component of a channel alone, and
`caelumora-open-terminal`, `caelumora-store-json` and `caelumora-store-postgres` bring backends
alone. What runs an agent or serves its facets is no plugin but a **library** of the agent side
(issue [516](../issues/516-the-runtime-trusts-the-agents-linked-into-it.md)), registered as a plugin
is, but with a builder that offers nothing of the kernel (`Library`, `LibraryBuilder`). The agents
are the essential one (issue 370): `caelumora-agents` registers the kinds of agent, whose actors the
channel builds (`AgentKinds`), and the kinds of their components; without it, the program is a chat
between users. `caelumora-mcp` ([TOOLS.md](TOOLS.md#mcp-servers)), `caelumora-a2a`,
`caelumora-compaction` and `caelumora-workspace-tools` bring component kinds alone,
`caelumora-comfyui` a section of the configuration and a service besides; they build on the agents'
library, and a library that defines facets of its own for others to serve is built on in turn:
`caelumora-skills` serves the `KnowledgeRestorer` of `caelumora-compaction`. A service with a
manager of its own that extends the kernel - skills and artifacts, which plugins bring, the
workspace, which the kernel brings itself - is a **system service**: it runs on the kernel's side,
its manager decides with the kernel's policies, its views serve the frontends, and what an agent's
work does with its kind it offers as extensions, a reader handed out where the work reads and a
writer where it acts ([PLUGINS.md](PLUGINS.md#what-a-system-service-offers-an-agents-work)). Which
plugin is trusted as the kernel and which is a library of the agent side, trusted for nothing, is
said in [Principles](#principles) and listed in [PLUGINS.md](PLUGINS.md#what-a-plugin-is). Where the
kernel calls a plugin, the plugin answers in the kernel's error types, not in its own: the kernel
decides by the types it knows what a person is told of an error.

Details: [PLUGINS.md](PLUGINS.md).

## Backends

Some services of the kernel are served by one of a choice of **backends**, named in the
configuration by their `type` (issue 324): the stores (`PersistenceBackend`, issue 446), the
storages of workspaces (`TerminalProvider`), the storages of binary content (`ContentStorage`, issue
282), the secret providers (`SecretProvider`) and the user providers (`UserProvider`); a plugin's
service may be served so too - the models of `caelumora-model` (`LanguageModelProvider`, issue 520),
which it makes of its section (`KernelBuilder::make_named`). A backend is made by the factory
registered under its type for its service, in `Backends` (`caelumora_kernel::backend`), which reads
its parameters and asks the services it is given for what it needs. The kernel registers its own
(`Backends::built_in`); a plugin adds one by registering its factory in `Plugin::backends`, which is
asked before anything is made - so a new storage or provider is a crate linked into the program, and
no edit of the kernel, as the Open Terminal storages of `caelumora-open-terminal` are (issue 327).
The backends are made once, when the kernel is built (`KernelBuilder::from_config`): the secret
providers first, which need only the policies, then the stores, which may ask for the secrets - a
database connects by one -, then the storages and the user providers with the secrets and the
records of the users at hand. The stores' backends are opened last, once the plugins registered and
opened the stores of their kinds (`PersistenceBackend::open`, issue 447), so that one that cannot be
made ready fails the start. The backends a section names by name are handed to their owners as one
`NamedBackends` - the storages of workspaces, the storages of content, a plugin's models -, of which
an owner uses the one named - where it keeps what is new, the model an agent's component names - or
the one there is: among several, one has to be named (`NamedBackends::chosen`). What is kept names
the backend it lies in and finds it again by that name (`NamedBackends::kept_in`); one since removed
from the configuration is `StorageGone`, the server's failure and not the request's. What a plugin
keeps beyond documents it keeps in the kernel's storages of content, which it is handed when it
registers (`KernelBuilder::content_storages`), in a key space of its own - the artifacts do. Unlike
a component, a backend serves the whole process and lives in no container, which is why it is no
component.

The stores are such backends since what a backend keeps is not generic: documents of any kind and
states, each offered by a method of its own (`PersistenceBackend`, issue 448); the stores, one
`Persistence`, are made of the backend of the entities and the backend of the states, and the kernel
opens its stores of them by the policies it is given, the only ones they check by (issue 449, see
[STORAGE.md](STORAGE.md#the-entity-store)).

Details: [CONFIGURATION.md](CONFIGURATION.md#backends).

## The domain

A **session** is one piece of work, such as a conversation about a topic. It has participants -
users and agents - and one or more **channels**, in which some of them talk. A channel is a chat
room with its own history, its own classification and its own **moderator**, which decides who is
asked to respond to a message.

Everything that takes part in a channel is an **actor**: a user (`UserActor`), an agent
(`AgentActor`), an agent that runs elsewhere (`ExternalAgentActor`), or the channel's approver
(`ApproverActor`), which collects the users' decisions on tool calls that need approval. Actors
communicate only through **envelopes** - an event with who sent it and in reply to what
(`EventContext`); on whose authority it is checked (`SecurityContext`) the channel builds from the
sender as it admits it.

An **agent** is an identity with a **personality**. The personality says what the agent consists of:
a list of components (its tools, its system prompt, its notes, ...), how its agents are introduced
to the others and how confidential its reasoning is. An agent is read through its personality, not
copied from it: what runs holds it through the instance's one `PersonalityController` of it (see
[STORAGE.md](STORAGE.md#following-changes)), and only an agent's components stay as they were
built. One agent can take part in many sessions and channels at the same time; it has one actor in
each channel and one state across them.

Sessions are made from **session templates**. A template names every participant of the session -
users, and agents with their personality and scope - and the channels, which refer to the
participants by name and name their internal actors, such as an approver. The domain builds the
session and its channels from the users it is given, without touching a store
(`SessionTemplate::build`); the kernel finds the owner, stores and starts the session, and then
finds or makes the agents - an agent is the one of its name in exactly the scope the template asks
for, made if there is none - and takes them in as anyone is taken into a running session; if that
fails, the session is taken back (`SessionManager::create_from_template`). REST and the TUI both
create sessions this way.

A session may have a **workspace** attached: a tree of files that belongs to a user, with one
classification, used by the participants of those channels of the session where it and the channel
flow both ways. Its files lie in a configured storage and are reached through a `Terminal`. What an
agent hands over to keep it publishes as an **artifact** (the plugin `caelumora-artifacts`): a fixed
copy of a file, in a version of an artifact that belongs to a user, its content kept in a storage
of content beyond the workspace and downloaded where it was published. Details:
[WORKSPACE.md](WORKSPACE.md), [TOOLS.md](TOOLS.md#artifacts).

Every session has a **project**, which its results go to: one of its own, made with it and gone with
it, or a shared one it lies in, which outlives its sessions. An artifact is published into its
session's project. Keeping a session's results is lifting its own project into its owner's scope or
the global one; a session is put into a shared project, or taken out, without anything moving.
Details: [PROJECTS.md](PROJECTS.md).

Sessions and channels hold their users and agents by reference - `Participant::User(UserId)`,
`Participant::Agent(AgentId)` - not as copies. The records live in the `EntityStore`; a channel
leaves out a participant whose record no longer exists, and runs without it. A deleted agent rests
in its channels so: they keep it, show it as a former participant and run it again once it is
restored. What of an actor outlives its run - the outbox a user follows a channel through, while the
channel is stopped too - is the `ActorManager`'s (issue 386). Making, deleting, restoring and
purging an agent are the `AgentManager`'s - a template's agents too, which the template only
finds -, renaming it its `AgentController`'s. What lies in a session's or a channel's scope goes
with it, deleted, restored and purged by the manager of its kind; so an agent is made there only by
whoever may read the session or the channel. An approver has no record of its own: it is held
whole by its channel, as an internal actor of it, no participant (issue 420).

Details: [CHANNELS.md](CHANNELS.md).

## How a message travels

The path of one message shows how the parts fit together:

1.  **A user writes.** The frontend (REST or TUI) hands the text to *that user's* actor in the
    channel. There is no way to write into a channel other than through one's own actor, so sender
    and authority always follow from who is writing.
2.  **The channel routes it.** The `ChannelLoop` resolves `@`-mentions, asks the moderator whom it
    goes to and whom to ask to respond, records it in the channel's history with its sender and whom
    it went to, and delivers it - and the requests to those asked - in the order of the history: the
    one record of what every participant received (issue 417).
3.  **An agent takes a turn.** The `AgentActor` begins a **turn**: its context provider assembles
    what the model is given (prompt parts, the agent's transcript), the model answers, and tool
    calls in the answer are run - after approval, where a tool requires it - and their results fed
    back, until the model answers without calling a tool.
4.  **The answer is published.** What the agent sends is stripped of what only its owner may see,
    delivered to the others through the channel, and the turn is committed: the messages go into the
    agent's transcript, the channel records them in full for the agent and as published for the
    others, staged changes to the agent's state are applied, and everything is stored.
5.  **Observers are told.** The channel's events reach the participants' live streams; changes to
    entities are announced on the event bus, which the web UI follows.

A turn that fails ends the turn, not the agent: it is reported as `TurnGivenUp` and the agent
returns to idle. A turn cut off by a crash is resumed when the agent is loaded again. Every turn has
an id, which `ResponseStarted` announces and every message of the turn carries.

What was said can be changed afterwards, but never rewritten: its author modifies or takes back a
message, and every store that holds it records the change after it, so that what an agent saw for an
earlier answer can still be told as it was. An agent can be asked to answer again instead of its
last answer; whether it does is its own to decide. Nothing forks. An answer still under way can be
stopped alone, or asked for again: its turn is then given up before any of it reaches the channel's
history, and the agent answers anew.

Details: [AGENT-TURN.md](AGENT-TURN.md), [CHANNELS.md](CHANNELS.md), [EVENTS.md](EVENTS.md),
[REST-SERVER.md](REST-SERVER.md#streams).

## Agents are made of components

What an agent can do is not programmed into the `AgentActor` but configured: a personality lists
**components**, and each component offers one or more **facets** - traits through which it takes
part in the turn. A component contributes tools (`ToolProvider`), text for the model
(`PromptProvider`), or filters incoming messages (`PerceiveInProcessor`). Of some facets an agent
has exactly one: the `LanguageModelClient`, the language model it talks to, and the
`ContextProvider`, which assembles the model's context. Both have built-in defaults that a
personality or the configuration can replace - the model's is the kind `language_model` of the
plugin `caelumora-model`, which talks to a model the platform offers (issue 520). Tool calls are run
by the agent's `ToolExecutor` with the tools of its providers; it is no component, so that what it
checks is never a personality's choice, and what of a call is the kernel's to decide - whether it
may be called, what the rules of its places say, what may flow where - it asks of the kernel
(`ActorRuntime::admit_tool_call`, `may_publish`, issue 516).

Components are wired by type, much like Spring beans: a component asks for the facets it needs, and
the container builds whatever provides them. User commands such as `/help` are components as well,
built once per session; a command that acts on an agent - `/compact` - is a component of its
personality, which the user's actor sends to the agent and the agent carries out in a turn of its
own.

Details: [COMPONENTS.md](COMPONENTS.md), [TOOLS.md](TOOLS.md), [APPROVALS.md](APPROVALS.md),
[COMMANDS.md](COMMANDS.md).

## Agents that run elsewhere

An agent need not run here. A personality of kind `external` makes an **external agent**: another
system with its own model, tools and memory, reached through the one component of the personality
that provides `ExternalAgent` - `a2a_agent`, of the library `caelumora-a2a`, speaks the Agent2Agent
protocol; the agents hold the facet alone and name no protocol. Its `ExternalAgentActor` has no
turn: it hears what is said in the channel, gives the remote agent what it has not heard yet when it
is asked, and says what comes back, also on its own when a task of the remote agent ends later. What
leaves the system is decided at the door: the roles its personality names limit what it is cleared
for, and an external agent that names none sees only what is public. What the remote agent was given
cannot be changed there; a message changed or taken back afterwards is told to it with its next
call, by a notice in the name of its author.

Details: [CHANNELS.md](CHANNELS.md#actors-and-envelopes),
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#personalities-limit-their-agents).

## What an agent remembers

An agent's memory has several tiers, each with one meaning: the channel's **history** (the record,
never rewritten, one for every participant), the agent's **transcript** (what its context is built
from), **notes** (published in the channel), **private notes** and **agent memory** (seen only by
the agent). An agent may also read the notes and history of other channels of its session, as far as
confidentiality allows. For every message an agent wrote, it can be reconstructed exactly what its
model saw, because the transcript is a chain of digests.

Details: [MEMORY.md](MEMORY.md), [AGENT-TURN.md](AGENT-TURN.md).

## What an agent can look up

What an agent knows how to do need not be built into its personality: a **skill** is a procedure
written down - instructions only, no tools - that a user owns, shares and edits like a document,
kept in the `EntityStore`. An agent with the `skills` component is shown a catalogue of the skills
it may load where it is - name and description - and loads one when a task calls for it; a
participant can give it one with `/skill`. Either way the skill's text goes into the agent's own
transcript, never into the system prompt or the channel's history, and loading it is a decision of
its own (`UseSkill`), besides whether its classification may flow into the channel. An agent may
also write a skill down, for its owner, with approval.

Details: [SKILLS.md](SKILLS.md).

## Reading what is there

What is in the system is read through one navigation, the **views** (`caelumora_kernel::view`):
from a session to its channels, from a channel to its history, notes and participants, from an agent
among them to what it saw - its transcript, its private notes, the context of each of its messages,
the turns it gave up. Each step asks its own access question, in the name of whoever reads; a node
is data that was allowed to be reached and decides nothing, so an operation still checks its own
permission. The views read what is stored - through the running loop where there is one - so running
and stopped sessions are read alike and reading starts nothing.

Two kinds of viewer walk the same steps: a user, who reads what they may read and are cleared for,
and an agent, which reads from its channel by the confidentiality rules, through the `Turn` its
components are called with. The REST server reads through the views and answers with their
snapshots; the components read through readers of them (`SessionReader`, issue 516), which offer
what an agent's work reads and nothing of the views' policies, managers or context; neither asks
the policy itself.

Details: [SECURITY.md](SECURITY.md#views), [MEMORY.md](MEMORY.md#reading-other-channels).

## State and persistence

Two stores, configured separately:

*   the **`EntityStore`** keeps long-lived documents - personalities, templates, the component
    bundles they include, agents, users and their profiles, sessions and channels, and the kinds of
    the plugins, one store per kind, which the backend opens for any kind;
*   the **`StateStore`** keeps what changes while a session runs - the channel's history, each
    agent's state in each channel, and each agent's own state -, the **`HistoryStore`** reads
    the histories the states hold, in pages, without loading a state, and the
    **`TranscriptStore`** what lies on the older branches of an agent's transcript, which its actor
    does not hold, and the prompt texts and tool sets its model was given (see
    [STORAGE.md](STORAGE.md#histories), [STORAGE.md](STORAGE.md#transcripts),
    [STORAGE.md](STORAGE.md#prompt-texts-and-tool-sets)).

A backend - in memory, JSON files - only keeps: documents of any kind and states, without asking
anything (`PersistenceBackend`). Who may do what with an entity is checked once, in
`caelumora-store`, for every backend alike; the `Persistence` is the stores over the backends of the
entities and of the states, checked by the policies they are opened by - the kernel's (see
[STORAGE.md](STORAGE.md#the-entity-store)).

Every document and every kind of state has exactly **one writer**, which stores a change and only
then announces it. For an entity, that is its manager or its controller, by what is changed
(below), and it writes on the revision it read: a write made on an outdated one is refused, and
the writer reads again and makes its change once more, so that no two writers overwrite each other,
in one process or several (issue 492). For state, the writer is the channel loop, the agent's actor
in its channel, or the agent's state handle, each changing it under a lock of its own. Agents and
users have one controller per process, however many sessions they take part in.

The server runs in one process today, but is cut so that it can run in several: a session runs in
exactly one instance, while agents and users may be active in several, kept in step through the
global events.

Details: [STORAGE.md](STORAGE.md).

## Managers and controllers

What changes an entity is split by what it changes:

*   **Its lifecycle is its manager's** - making it, deleting, restoring and purging it, with what
    lies in its scope: the `SessionManager`'s for sessions, the `AgentManager`'s, `GroupManager`'s
    and `WorkspaceManager`'s for their kinds, the `ProjectManager`'s for shared projects - made by
    the program beside the `SessionManager`, which it holds -, and those of the plugins
    (`SkillManager`, `ArtifactManager`). A channel's is its session's (`SessionLoop`); a session's
    own project is made, deleted, restored and purged with the session by the `SessionManager`.
*   **What it is - its attributes - is its controller's**, where it has one: a name, a description,
    rules, participants. Sessions, channels, agents, users, workspaces and projects have one, as
    running sessions hold them live; the controller stores a change and announces it, so that the
    live copy and the store do not disagree.
*   **An entity without a controller** - a group, a skill, an artifact - is held live by nobody,
    and its manager changes its attributes too. A controller is made for a kind once
    something holds it live, not for the sake of this split.

An attachment is no entity: it is part of its message, read and taken back with it. The lifecycle
of its content is the `AttachmentManager`'s: kept as it is uploaded for a message - or copied, as an
agent attaches what it heard -, claimed by the message as the channel admits it, and removed with
the channel's history (issue 282). What a tool gave its model is kept the same way, but for its
result, which refers to it at once: claimed by none (issue 512).

## Security and confidentiality

Two questions are kept apart:

*   **Who may do what** is decided by one `AccessPolicy` per process, asked with an `ActionType` and
    the `SecurityAttributes` (owner and access list) of the entity concerned. The default policy is
    ownership: owners do everything, whom the access list grants it read and use - a user, a role,
    or a group: a team users make, the participants of a session or channel, or the members of a
    project, which follow who belongs to it. What a client may see, it reads through the kernel's
    **views** (see [Reading what is there](#reading-what-is-there)), which ask the policy at every
    step for that viewer; frontends never ask the policy themselves. Users come from a chain of user
    providers, and the REST server authenticates them with a short-lived signed token.
*   **What may flow where** is decided by classifications, not by who takes part. Every channel has
    a classification - a level from `public` to `strictly_confidential` and an audience of
    compartments, which the administrator configures - and one flow rule decides whether a channel
    may read another. Apart from that, the parts of a message are flagged if only the agent's owner
    may see them: the channel records them, and gives them to nobody else.

Details: [SECURITY.md](SECURITY.md), [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

## Principles

The design repeats a few decisions; new code follows them.

*   **The kernel decides, frontends map.** What a user may see, which commands exist, whether a line
    is a message or a command - each is decided in one place in the kernel or the domain. REST and
    the web UI translate it and repeat no rule.
*   **One writer per document.** No state is changed from two places; changes are stored first and
    announced afterwards, and events only announce - nothing changes because an event arrived.
*   **Stage, then commit.** A turn stages its changes and applies them when it completes, so a
    failed turn leaves nothing behind.
*   **Least authority.** Everything is handed only what it needs: a component gets a narrow context,
    not its actor's runtime; an actor gets its own channel, not the session.
*   **The kernel decides, the agent side asks.** As the frontends map what the kernel decides, the
    libraries of the agent side ask it. The kernel is to the agent side what an operating system's
    kernel is to its user space - a modular one: its own services are sessions, channels with their
    history, the attachments of their messages and their approver, actors and their state, and
    persistence, with what they need around them - security with the secrets bound to a context, the
    event bus, projects -; the models the platform offers are a plugin's service beside it, needed
    within an agent alone (issue 520); the workspace, skills, artifacts and memory are system
    services that extend it (issue
    [516](../issues/516-the-runtime-trusts-the-agents-linked-into-it.md)) - so far the workspace,
    skills and artifacts as extensions; memory is still reached through the agent's state (issue
    [518](../issues/518-memory-is-reached-through-the-agents-state.md)). A plugin that brings what
    is the kernel's - a backend, a kind of entity with its manager, views and routes, a component of
    a channel - is trusted as the kernel; what runs an agent or serves its facets is a **library**
    of the **agent side**, trusted for nothing: `caelumora-agents`, and the crates that bring
    components for an agent (which crate stands where is listed in
    [PLUGINS.md](PLUGINS.md#what-a-plugin-is)). A library is not a program: the binaries
    (`caelumora-app`, `caelumora-tui`) link the kernel, its plugins and the libraries, and choose
    which are in. A library sees the kernel through the kernel's API alone - an actor its
    `ActorRuntime`, a component its `ComponentContext` and the `Turn` it is called with -, and the
    kernel checks every call through it in the name of the actor it gave it to. A library builds no
    `SecurityContext`, asks no policy and holds no manager, no store and no controller of the
    kernel: what it needs decided, it asks the kernel to decide. What the kernel cannot enforce it
    does not pretend to: a tool whose effect lies outside the system runs if the library runs it,
    and an agent's own record - its transcript - is the agent side's; the kernel's record of a
    conversation is the channel's history, taken at its door. A library registers with a
    `LibraryBuilder`, which offers nothing of the kernel, and the kernel is not built where a
    component kind of a library provides a facet the kernel builds in its own name or uses a service
    of the kernel but the texts (issue
    [516](../issues/516-the-runtime-trusts-the-agents-linked-into-it.md)). A library links the API,
    the domain and other libraries, never the kernel, `caelumora-security` or `caelumora-store` -
    `scripts/library-deps.sh` checks its dependencies, run by the pre-commit hook -, so the
    compiler refuses it what decides: a view, a manager, a controller, the runtime of a
    participant, the `KernelBuilder`, a `SecurityContext`, a policy, the `SecretManager`, a store
    (issue [517](../issues/517-the-model-offers-libraries-what-only-the-kernel-may-decide-by.md)).
*   **Refusals do not reveal.** Something the caller may not see is reported exactly like something
    that does not exist, so ids and names cannot be probed.
*   **Records are not recomputed.** Digests, resolved mentions, whom an entry of a history went to
    and what a newcomer was given of it are fixed when they arise and stored, so a later change of
    scheme, name or attendance cannot rewrite the past.

## Documents

*   [CRATES.md](CRATES.md) - how the crates depend on each other, and the rules the graph keeps.
*   [COMPONENTS.md](COMPONENTS.md) - components, facets, how they are wired, the built-in
    components.
*   [TOOLS.md](TOOLS.md) - tools and how their calls run.
*   [APPROVALS.md](APPROVALS.md) - approvals of tool calls, and questions a tool asks a person.
*   [COMMANDS.md](COMMANDS.md) - user commands and the commands an agent carries out.
*   [AGENT-TURN.md](AGENT-TURN.md) - the agent's cognitive cycle, the turn, what an agent saw,
    prompt parts.
*   [MEMORY.md](MEMORY.md) - notes, private notes, agent memory, reading other channels, the terms
    for what an agent remembers.
*   [SKILLS.md](SKILLS.md) - skills: what they are, who may use and write them, how they reach the
    model.
*   [CHANNELS.md](CHANNELS.md) - sessions, channels, actors, participants, moderators and mentions.
*   [PROJECTS.md](PROJECTS.md) - projects: what outlasts a session, where artifacts lie.
*   [EVENTS.md](EVENTS.md) - the event bus.
*   [REST-SERVER.md](REST-SERVER.md) - the REST API, authentication, errors and the server-sent
    event streams.
*   [WEB-UI.md](WEB-UI.md) - the browser frontend: how it talks to the server and what it shows.
*   [STORAGE.md](STORAGE.md) - entity and state store, controllers, several instances.
*   [WORKSPACE.md](WORKSPACE.md) - workspaces, their storages and the commands run in them.
*   [SECURITY.md](SECURITY.md) - the access policy, ownership, views, users, authentication,
    secrets.
*   [CONFIDENTIALITY.md](CONFIDENTIALITY.md) - classifications, compartments and the flow rule, what
    only an owner sees, and what is still open.
*   [PERMISSION-CHECKS.md](PERMISSION-CHECKS.md) - the map of every permission check: which
    operation asks what, in whose name, by which concept, and where it is asked again.
*   [CONFIGURATION.md](CONFIGURATION.md) - the sections of the configuration file.
*   [LLM-MODELS.md](LLM-MODELS.md) - which settings suit which language model.
*   [CODING-GUIDELINES.md](CODING-GUIDELINES.md) - coding style, error handling, logging, tests.
