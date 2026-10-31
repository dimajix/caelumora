# Plugins

A plugin brings a subject the core does not know - a kind of entity, its components, its views, its
routes -, or a backend of a service of the core, in a crate of its own. This document describes what
a plugin is and how it registers, what of the runtime a component kind outside the core may use, and
how a plugin answers the core when something fails. How components are declared and built is in
[COMPONENTS.md](COMPONENTS.md); the overall picture, with the plugins there are, in
[ARCHITECTURE.md](ARCHITECTURE.md#plugins).

## What a plugin is

A plugin (issue 323) is a crate linked into the program that brings a subject the core does not
know - a kind of entity with what keeps it, its components, its views and its routes - or a backend
of a service of the core (issue 324). It implements `caelumora_runtime::plugin::Plugin` - a name for
messages, `backends` and `register` - and the program lists it: `caelumora-app` in `rest_plugins()`
if it serves routes, otherwise in `plugins()`, and builds its runtime with both; `caelumora-tui` in
its own `plugins()`. Both list the agents' plugin first, the essential one (issue 370). A program
builds its runtime with `RuntimeBuilder::from_config`, given the configuration and its plugins, in
two steps:

1.  Each plugin registers its backends (`backends`, see
    [ARCHITECTURE.md](ARCHITECTURE.md#backends)) with the core's (`Backends::with_plugins`), before
    anything is made: what the core's services are made of has to be known first.
2.  The policies, the stores and the secrets are made, then the core's services, managers and
    component kinds (`RuntimeBuilder::new`), with the backends the configuration names; then each
    plugin registers the rest of what it brings (`register`) - the agents' plugin the kinds of an
    agent's components and the kinds of agent (issue 370) -, and `build` makes the runtime. The
    program names the default kinds of singles after its plugins registered (`set_default_kind`).

A plugin stands on one of two levels of trust (see [ARCHITECTURE.md](ARCHITECTURE.md#principles)).
One that brings what is the core's - a backend, a kind of entity with its manager, views and routes,
a component of a channel such as the `ResponderPolicy` of `caelumora-decision` - is trusted as the
core and is given what the core has: the stores, the policies, the secrets, the controllers, the
system's context. One that runs an agent or brings components for an agent's facets is a library of
the agent side, trusted for nothing: the agents' plugin, `caelumora-mcp`, `caelumora-a2a`,
`caelumora-ollama`, `caelumora-comfyui`, `caelumora-workspace-tools` and `caelumora-compaction`.
`caelumora-skills` and `caelumora-artifacts` stand on both: their manager, views and routes are the
core's, their components a library's. A library reaches the core through the interface the core
gives an actor and its components (see
[What a component kind outside the core may use](#what-a-component-kind-outside-the-core-may-use)),
and may offer a service of its own to its own kinds - `caelumora-comfyui` its workflows -, which
decides nothing of the core's. Both levels register as a `Plugin`, and "plugin" in this document
covers both, until libraries register on their own (issue
[516](../issues/516-the-runtime-trusts-the-agents-linked-into-it.md)).

A test builds the runtime with `testing::TestRuntime`, naming the plugins it needs (issue 375). A
test of the agents' logic names the agents' plugin; the runtime's own tests name scripted agents
(`testing::ScriptedAgents`, issue 370) where an agent takes part, so that they test what the runtime
does with an agent, not what an agent does. A backend's factory is registered with
`Backends::register` for the service it serves (`dyn TerminalProvider`, say) and the struct of its
parameters, which implements `Params` - `NoParams` for a type that takes none -, is given them read
and checked, one it does not take refused before it is called, and asks the services it is given -
the policies, the `SecretManager`, the store of the users - with `service_of`. While it registers,
a plugin

*   asks for the store of its kind (`entity_store::<T>()`), checked by the runtime's policies. The
    configured backend keeps any kind in one storage - whoever asks for the kind again is given a
    store over the same storage - and a kind whose name another kind has is refused;
*   reaches the core's services it needs for its manager - the event bus, the policies, the secrets,
    the controllers - and offers its manager as a service (`provide`), which its views read through
    (`Views::service`) and its routes take when they are built; what an agent's work does with its
    kind it binds to the views of that work (`extend_readers`, `extend_writers`, below);
*   registers its component kinds (`components()`), as `register_all` does for the core's;
*   adds the texts it gives models to the prompt catalogue (`prompts()`, issue 57), under a
    namespace of its own, from a file it compiles in - the agents' `agents`, from
    `caelumora-agents/src/prompts.yaml`; `skills`, `compaction`, `workspace_tools`, `artifacts`
    and `decision` from a `src/prompts.yaml` of their own -; its component kinds that write for a
    model use the catalogue as a service (see
    [AGENT-TURN.md](AGENT-TURN.md#the-prompt-catalogue)). A tool that names another plugin's tool
    writes its name as text: it does not know it as a value;
*   registers the kinds of agent it brings (`agent_kinds()`, an `AgentKinds`): for a kind of
    personality, what builds its agents' actors - from the agent's `ActorRuntime` alone, issue
    516 - and which facets their components are asked for.
    The agents' plugin (`AgentsPlugin`, issue 370) registers both kinds there are; a program
    without it runs no agent;
*   is handed the core's storages of content (`content_storages`, issue 282) for what it keeps
    beyond documents - the versions of artifacts -, under a key space of its own;
*   makes its kind go with what holds the scope it lies in (`scope_member`, a `ScopeMember`, which
    says only what its kind adds - where it announces and what lies behind an entity), and
    puts it into a scope only by the holders of scopes (`scope_holders`, a `ScopeHolders`): asked
    before it writes (`require`) and once it has written (`keep_made`, `restore`), so that
    nothing is left in the scope of a holder deleted meanwhile (issue 458);
*   claims its section of the configuration (`config::<C>(name)`), read as `C`, and of the seed
    files (`seed_section`, a `SeedSection`). Both sections stand at the top level of their files;
    one that no plugin claims is an error, so that a misspelt section is found.

Its views are an extension trait on `Views`, built from what `Views` opens: the viewer's `context`,
the `access_policy` and `confidentiality` policies, its manager as a `service`, `channel_reading`,
`deleted_of`, by which it lists what of its kind was deleted, as every kind is listed, and
`scope_snapshot`, by which it answers where something of its kind lies, as every kind answers it.
What a channel's view lists of its kind - the artifacts published in it, say - is an extension trait
on `ChannelView`, which opens its `views` and how its viewer reads it (`reading`): whether the
viewer reads the channel stays the channel's to decide (issue 328). What lies in a project is listed
the same way, by an extension trait on `ProjectView` (issue 467). What an agent's work does with its
kind - loads a skill, publishes an artifact - the plugin offers as its extensions, a reader and a
writer (see
[What a system service offers an agent's work](#what-a-system-service-offers-an-agents-work)).
Its routes come from `caelumora_rest_server::plugin::RestPlugin`, a `Plugin` that builds a `Router`
with the `AppState`; the API serves them behind the same login as the core's (`routes::api`). A
plugin that brings component kinds alone, such as `caelumora-mcp` (issue 325), `caelumora-a2a`
(issue 326), `caelumora-ollama` (issue 329), `caelumora-decision` (issue 403),
`caelumora-workspace-tools` (issue 428) or `caelumora-compaction` (issue 464), implements `Plugin`
only and registers the kinds in `register` - as does one that claims a section of the configuration
for them besides, `caelumora-comfyui` (issue 512); whether a kind of a facet of the core of which
there is one is its default, the program says, not the plugin
(`ComponentRegistry::set_default_kind`), so that two plugins of a facet cannot both claim it. A
facet a plugin brings itself is its own: `caelumora-compaction` registers its `CompactionStrategy`
as a single with its default kind, as the core's modules do theirs. One that brings backends alone,
such as `caelumora-open-terminal` (issue 327) or `caelumora-store-json` (issue 446), registers them
in `backends` and nothing in `register`. A backend of the stores runs the store suite of the model
in its own tests (`caelumora_model::store::suite`, feature `test-support`). A plugin's tests use the
world of the security suite - the runtime with the recording ownership policy and three users -,
`caelumora_runtime::testing`, as a dev-dependency with the feature `test-support`. That world runs
no agent; a plugin whose components serve an agent's facets tests them with the agents beside it -
`World::with(&[&TestAgents, &Plugin], …)` - where `caelumora_agents::testing::TestAgents` brings the
agents' plugin and a model that is not to be asked as the default (issues 370, 373). Naming that
default is the program's, and in a test, `TestAgents` stands in for the program.

## What a system service offers an agent's work

A service with a manager of its own that extends the core - skills and artifacts, which plugins
bring; the workspace, which the core brings itself - is a **system service** (issue
[516](../issues/516-the-runtime-trusts-the-agents-linked-into-it.md)): it runs on the core's side,
its manager decides and keeps, asking the core's policies extended by questions of its own
(`SkillAccess`), never rules of its own, and its views serve the frontends. What an agent's work
does with its kind it offers as **extensions**, by one pattern:

*   **Two faces.** What the work reads by is a **reader** (`SkillReader`), handed out where it reads
    (`Situation::reader`); what it acts by is a **writer** (`SkillWriter`, `ArtifactPublisher`),
    handed out where it acts (`Turn::writer`). So a prompt part cannot write a skill, as the
    workspace's terminal is read through the situation and changed through the turn. Whether there
    is a writer, without being given it, the situation answers (`has_writer`) - for a tool provider
    that offers what acts by it.
*   **Bound by the plugin, per work.** The plugin - the core for its own, the workspace - binds each
    face to every piece of work (`RuntimeBuilder::extend_readers`, `extend_writers`), once: a type
    bound already is refused, so that none replaces what another binds. A binding is given the
    work's session as the work reads it (`SessionView`), reached by the core asking `GetSession`,
    makes the face from its views - as the plugin's own views are built - so that it acts in the
    work's context, may make it asynchronously, and may answer none where it is not offered - the
    artifacts' writer where no storage of content is configured, the workspace's reader where the
    work may use none. No method of a face takes a context or an identity, and what it hands out is
    data, not an object of the core - but for a handle, named as one, that carries a right beyond
    the call: the workspace's terminal and shell.
*   **Asked for where it is used, never kept.** A face acts in the context of the call that asked
    for it - the rights as they are at each call -, so a component asks for it again each time.
*   **Declared.** A component kind declares the faces it reads and writes by
    (`ComponentKind::reads`, `writes`); a runtime in which a declared face is bound by no plugin is
    not built. The declaration guards the build alone: a face is handed to whichever component asks
    for it, as the situation and the turn know no kinds.
*   **Named alike.** A reader is a `…Reader`, a writer a `…Writer`, unless what it does names it
    better (`ArtifactPublisher`).

## What a component kind outside the core may use

A component kind in a crate of its own uses the runtime through what a component of the core uses
too, and nothing else - there is no prelude that re-exports at large, so that it shows which types
a plugin depends on:

*   **To register it:** `ComponentRegistry::register` or `register_async`, `ComponentKind` with the
    parameters it takes (`params`, a struct that implements `Params`), the facets it `provides`,
    those of others it `needs`, the services it `uses` and the extensions it `reads` and `writes`
    by, `ComponentFacets`, and `config_of_params` to read its `params`. A kind that declares no
    parameters takes none: every one it is given is refused
    ([COMPONENTS.md](COMPONENTS.md#parameters)).
*   **While it is built:** the `ComponentContext` - in whose name it is built, the core's to set and
    not given out (issue 516) -, `service::<T>()`, the services of the runtime it declared, and
    `all` and `one`, the facets of others it declared it needs. What a container accepts follows
    from these declarations ([COMPONENTS.md](COMPONENTS.md#what-a-container-accepts)). A service
    that decides for the core - a policy, one of the core's managers, a store - is the core's, and a
    component of a library does not declare it: what it needs decided it asks of the core, through
    the `Turn` and the `Situation` it is called with (`may_reclassify`, `may_copy_information`,
    `copy_attachment`; the executor's `admit_tool_call`, issue 516), and what it reads it reads
    through the readers (`Situation::session`, `SessionReader`). What it needs of another plugin it
    declares as the trait that plugin offers for it, or, of a system service, as the extension it
    reads or writes by: a reader its situation hands out (`Situation::reader`), a writer its turn
    (`Turn::writer`).
*   **The facets it offers**, with what their signatures name: for a tool provider `ToolProvider`,
    `ToolOutput`, `ToolError` and `ToolResult`, `ApprovalMode`, and the `Turn` a call is made in;
    for an external agent `ExternalAgent`, `OutboundMessage` and `Said`, `ExternalEvent`,
    `ExternalStream`, `ExternalAgentError` and `ExternalAgentResult`.
*   **If it talks to a remote system:** `caelumora_runtime::remote`, which describes its errors
    as the core's are described - `describe_request_error`, `describe_chain` (issue 326) -, and
    the `Secrets` its credential is read of (`RemoteEndpoint::bearer_token`): the work's, from its
    `Situation` (`secrets`), or handed to an `ExternalAgent` with every call (issue 516).
*   **The model's types** of what it handles: `Tool`, `RemoteEndpoint`, `Classification`, ...

What a plugin needs beyond that is made visible when a plugin needs it, as part of the runtime's
API, rather than reached around (issue 325).

## Errors: a plugin answers the core in the core's types

Where the core calls a plugin - a facet of its component, a `ScopeMember` - the plugin answers with
an error type of the core, never with one of its own inside an `anyhow` (issue 336). The core
decides what a person is told of an error by the types it knows -
`caelumora_runtime::error::user_message` for the user of a command and the TUI, and
`From<anyhow::Error> for ApiError` for the REST API -, and a plugin's types are not among them:
inside an `anyhow`, whatever a plugin's error says is shown as a failure that points to the log.
The agents' plugin, which defines the facets most plugins serve, is a core of its own in this: what
an agent's owner is told when an answer fails is judged by `caelumora_agents::error::user_message`,
which knows
the error types of those facets and asks the runtime's judgement for the rest
(`error::user_message_with`, issue 370). A registry of judgements the plugins add is not needed,
and would be the registry of downcasts rejected below. A plugin may define a facet in turn that
other plugins serve - `caelumora-compaction` the `KnowledgeRestorer`, which the `skills` component
serves (issue 464) -: they depend on it and answer in its error type, `CompactionError`, as they
answer the agents in theirs; what is shown of it, the defining plugin decides where it answers the
agents - the `compaction` component carries its text into its `ActivityError` and `CommandError`.

So the trait names its error type, and the plugin translates its own error into it where it
answers - deciding there what may be shown, what is a refusal and what is a failure:

*   a tool provider answers `ToolError`, an agent's command `CommandError`, a knowledge restorer
    `CompactionError`, an external agent `ExternalAgentError`, an agent's activity
    `ActivityError`;
*   a prompt provider and a context provider answer `prompt::ContextError`, a message processor
    `message::ProcessingError` - each with `Failed`, a failure in words that may be shown, and the
    view's and the store's errors, judged as those are;
*   a `ScopeMember` answers `StoreResult` for what lies behind an entity (`remove_behind`) - a
    workspace's place, an artifact's content -, which is its storage too: a failure to remove it is
    a `StoreError::Backend`;
*   a responder policy answers `moderator::Undecided` when it could not choose, in words for the
    log alone, without anything that was said.

The skills' component turns a `SkillError` into a `ToolError` for its tools, into a `CommandError`
for its commands, into a `CompactionError` when the context is compacted and into a `ContextError`
for its catalogue. The compiler asks for the translation, which a `?` into an `anyhow` would let be
forgotten.

The plugin's own error type stays within the plugin: its manager, its views and its routes, whose
handlers answer it by a `From<…> for ApiError` of the plugin's (see
[REST-SERVER.md](REST-SERVER.md#errors)). `Plugin::register` and `SeedSection::seed` answer
`anyhow`: they run at startup, and what fails there ends the program and goes to the log.

What is not done instead, and why: a type of the core carried as `#[source]` that marks a message as
safe to show, after the pattern of `Refused` - but a refusal is what caused the error carrying it,
and a mark of what may be shown is no cause -; and a trait such as `UserFacing` the core finds in
the chain - a `&dyn Error` cannot be asked for a trait of its type, so it would take a registry of
downcasts beside the chain.
