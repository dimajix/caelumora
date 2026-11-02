# Plugins

A plugin brings a subject the kernel does not know - a kind of entity, its components, its views,
its routes, a service of its own -, or a backend of a service of the kernel or of another plugin, in
a crate of its own; a library brings what runs an agent or serves its facets, on the agent side.
This document describes what a plugin is and how it registers, what a library is and how it
registers, what of the kernel a component kind outside the kernel may use, and how a plugin answers
the kernel when something fails. How components are declared and built is in
[COMPONENTS.md](COMPONENTS.md); the overall picture, with the plugins there are, in
[ARCHITECTURE.md](ARCHITECTURE.md#plugins).

## What a plugin is

A plugin (issue 323) is a crate linked into the program that brings a subject the kernel does not
know - a kind of entity with what keeps it, its components, its views and its routes, or a service
of its own (`caelumora-models`, issue 520) - or a backend of a service of the kernel (issue 324) or
of another plugin (`caelumora-ollama`). It implements `caelumora_kernel::plugin::Plugin` - a name
for messages, `backends` and `register` - and the program lists it: `caelumora-app` in
`rest_plugins()` if it serves routes, otherwise in `plugins()`, and builds the kernel with both;
`caelumora-tui` in its own `plugins()`. A program builds the kernel with
`KernelBuilder::from_config`, given the configuration and its plugins, in two steps, and registers
its libraries after (see [What a library is](#what-a-library-is)):

1.  Each plugin registers its backends (`backends`, see [ARCHITECTURE.md](ARCHITECTURE.md#backends))
    with the kernel's (`Backends::with_plugins`), before anything is made: what the kernel's
    services are made of has to be known first.
2.  The policies, the stores and the secrets are made, then the kernel's services, managers and
    component kinds (`KernelBuilder::new`), with the backends the configuration names; then each
    plugin registers the rest of what it brings (`register`). Once the libraries registered too,
    `build` makes the kernel.

A plugin brings what is the kernel's - a backend, a kind of entity with its manager, views and
routes, a component of a channel such as the `ResponderPolicy` of `caelumora-decision` - and is
trusted as the kernel: it is given what the kernel has, the stores, the policies, the secrets, the
controllers, the system's context. What runs an agent or brings components for an agent's facets is
no plugin but a library of the agent side, trusted for nothing (see
[What a library is](#what-a-library-is) and [ARCHITECTURE.md](ARCHITECTURE.md#principles)).
`caelumora-skills` and `caelumora-artifacts` stand on both levels, cut by Cargo features (issue
[516](../issues/516-the-runtime-trusts-the-agents-linked-into-it.md)): what an agent's work does
with their kind is the crate's part of the API (`api`: the faces and their errors), their manager,
views and routes are the kernel's - a plugin (`kernel`, `server`: `SkillsPlugin`,
`ArtifactsPlugin`), which binds the faces -, and their components a library's (`library`:
`SkillsLibrary`, `ArtifactsLibrary`). Each side builds without the other - the plugin without the
agents (`kernel` alone), the library without the plugin (`library` alone), the entity and the wire
types without either -, which the pre-commit hook checks. A program lists both; the library alone
keeps the kernel from being built, as no plugin binds what its component reaches.

A test builds the kernel with `testing::TestKernel`, naming the plugins and the libraries it needs
(issue 375). A test of the agents' logic names the agents' library; the kernel's own tests name
scripted agents (`testing::ScriptedAgents`, a library, issue 370) where an agent takes part, so that
they test what the kernel does with an agent, not what an agent does. A backend's factory is
registered with `Backends::register` for the service it serves (`dyn TerminalProvider`, say) and the
struct of its parameters, which implements `Params` - `NoParams` for a type that takes none -, is
given them read and checked, one it does not take refused before it is called, and asks the services
it is given - the policies, the `SecretManager`, the store of the users - with `service_of`. While
it registers, a plugin

*   asks for the store of its kind (`entity_store::<T>()`), checked by the kernel's policies. The
    configured backend keeps any kind in one storage - whoever asks for the kind again is given a
    store over the same storage - and a kind whose name another kind has is refused;
*   reaches the kernel's services it needs for its manager - the event bus, the policies, the
    secrets, the controllers - and offers its manager as a service (`provide`), which its views read
    through (`Views::service`) and its routes take when they are built; what an agent's work does
    with its kind it binds to the views of that work (`extend_readers`, `extend_writers`, below);
*   registers its component kinds (`components()`), as `register_all` does for the kernel's;
*   adds the texts it gives models to the prompt catalogue (`prompts()`, issue 57), under a
    namespace of its own, from a file it compiles in - `decision` from its `src/prompts.yaml`, as a
    library adds its own (`LibraryBuilder::add_prompts`): the agents' `agents`, from
    `caelumora-agents/src/prompts.yaml`; `skills`, `compaction`, `workspace_tools`, `artifacts` and
    `comfyui` from a `src/prompts.yaml` of their own -; its component kinds that write for a model
    use the catalogue as a service (see [AGENT-TURN.md](AGENT-TURN.md#the-prompt-catalogue)). A tool
    that names another plugin's tool writes its name as text: it does not know it as a value;
*   is handed the kernel's storages of content (`content_storages`, issue 282) for what it keeps
    beyond documents - the versions of artifacts -, under a key space of its own;
*   makes the backends its own service is served by, which other plugins may bring, from its section
    (`make_named`, issue 520) - the models of `caelumora-models`, served by the backend `ollama` of
    `caelumora-ollama`;
*   makes its kind go with what holds the scope it lies in (`scope_member`, a `ScopeMember`, which
    says only what its kind adds - where it announces and what lies behind an entity), and
    puts it into a scope only by the holders of scopes (`scope_holders`, a `ScopeHolders`): asked
    before it writes (`require`) and once it has written (`keep_made`, `restore`), so that
    nothing is left in the scope of a holder deleted meanwhile (issue 458);
*   claims its section of the configuration (`config::<C>(name)`), read as `C`, under `plugins`, and
    of the seed files (`seed_section`, a `SeedSection`), at their top level - the seed holds what
    the plugins keep, and no library claims a section of it, so that it has nothing to keep apart;
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
[What a system service offers an agent's work](#what-a-system-service-offers-an-agents-work)). Its
routes come from `caelumora_rest_server::plugin::RestPlugin`, a `Plugin` that builds a `Router` with
the `AppState`; the API serves them behind the same login as the kernel's (`routes::api`). A plugin
that brings a component of a channel alone, such as `caelumora-decision` (issue 403), registers its
kind in `register`. One that brings backends alone, such as `caelumora-open-terminal` (issue 327) or
`caelumora-store-json` (issue 446), registers them in `backends` and nothing in `register`. A plugin
may depend on another plugin to bring a backend of that plugin's service - `caelumora-ollama` on
`caelumora-models` (issue 520) -, as it depends on the kernel to bring one of the kernel's; what a
plugin's service offers is its own API, as the faces of `caelumora-skills` are. A backend of the
stores runs the store suite in its own tests (`caelumora_store::suite`, feature `test-support`). A
plugin's tests use the world of the security suite - the kernel with the recording ownership policy
and three users -, `caelumora_kernel::testing`, as a dev-dependency with the feature `test-support`.
That world runs no agent unless a library brings them (`World::with(plugins, libraries, …)`).

## What a library is

A library (issue [516](../issues/516-the-runtime-trusts-the-agents-linked-into-it.md)) is a crate of
the agent side: what runs an agent - `caelumora-agents`, the essential one, which brings the kinds
of agent and the built-in components (issue 370) - or brings components for an agent's facets -
`caelumora-mcp` (issue 325), `caelumora-a2a` (issue 326), `caelumora-workspace-tools` (issue 428),
`caelumora-compaction` (issue 464), `caelumora-comfyui` (issue 512), and the library sides of
`caelumora-skills` and `caelumora-artifacts` (`SkillsLibrary`, `ArtifactsLibrary`). It is linked
into the program as a plugin is, but trusted for nothing, and links the kernel's API
(`caelumora-api`), the model and other libraries, never the kernel, `caelumora-security` or
`caelumora-store`: they are dev-dependencies of its tests alone, which build the kernel's world -
for `caelumora-agents` its `testing` too, under `test-support` -, which `scripts/library-deps.sh`
checks, run by the pre-commit hook. It implements `caelumora_api::library::Library` - a name for
messages and `register` -, and the programs list it in `libraries()`, the agents' library first, and
register each with `KernelBuilder::library` once their plugins registered. A library registers with
a `LibraryBuilder`, a trait of the API the kernel implements, which offers what a library registers
and nothing of the kernel - no store, no policy, no secret, no controller:

*   its component kinds (`components()`, a `LibraryComponents`), marked as a library's;
*   its kinds of agent (`register_agent_kind`): for a kind of personality, what builds its agents'
    actors - an `AgentFactory`, from the agent's `ActorRuntime` alone (`agent_factory`) - and which
    facets their components are asked for. Kinds of agent are the agent side's: a plugin registers
    none. The agents' library registers both kinds there are; a program without it runs no agent;
*   its texts (`add_prompts`), under a namespace of its own, as a plugin adds its - and nothing else
    of the catalogue: its variants are the configuration's;
*   its section of the configuration (`config::<C>(name)`, over `section`), under `libraries`;
*   a service of its own to its own kinds (`provide`, over `provide_of`), which decides nothing of
    the kernel's - `caelumora-comfyui` its workflows.

The typed calls (`config`, `provide`) lie on `dyn LibraryBuilder`, the trait's erased ones beside
them (`section`, `provide_of`), as `ActorRuntime::reader` lies over `reader_of`.

The kernel is not built where a kind of a library provides a facet the kernel builds in its own
name, a moderator of a channel or a command of a session, or uses a service that is neither a
library's nor the texts (`PromptCatalogue`): a library's components reach nothing of the kernel but
through the interface they are given. A facet a library brings itself is its own:
`caelumora-compaction` registers its `CompactionStrategy` as a single with its default kind. A
library whose components serve an agent's facets tests them with the agents beside it -
`World::with(&[], &[&AgentsLibrary, &Library], …)` -, whose kernel gives its agents a model that is
not to be asked unless a test brings the models' plugin (`testing::with_unasked_model`, issues 370,
373, 520). A library may bring a model of its own (a `ModelClient`): what it costs is then the
agent's affair, and its credentials only the secrets the kernel lets it read.

## What a system service offers an agent's work

A service with a manager of its own that extends the kernel - skills and artifacts, which plugins
bring; the workspace, which the kernel brings itself - is a **system service** (issue
[516](../issues/516-the-runtime-trusts-the-agents-linked-into-it.md)): it runs on the kernel's side,
its manager decides and keeps, asking the kernel's policies extended by questions of its own
(`SkillAccess`), never rules of its own, and its views serve the frontends. What an agent's work
does with its kind it offers as **extensions**, by one pattern:

*   **Two faces.** What the work reads by is a **reader** (`SkillReader`), handed out where it reads
    (`Situation::reader`); what it acts by is a **writer** (`SkillWriter`, `ArtifactPublisher`),
    handed out where it acts (`Turn::writer`). So a prompt part cannot write a skill, as the
    workspace's terminal is read through the situation and changed through the turn. Whether there
    is a writer, without being given it, the situation answers (`has_writer`) - for a tool provider
    that offers what acts by it.
*   **Bound by the plugin, per work.** The plugin - the kernel for its own, the workspace - binds
    each face to every piece of work (`KernelBuilder::extend_readers`, `extend_writers`), once: a
    type bound already is refused, so that none replaces what another binds. A binding is given the
    work's session as the work reads it (`SessionView`), reached by the kernel asking `GetSession`,
    makes the face from its views - as the plugin's own views are built - so that it acts in the
    work's context, may make it asynchronously, and may answer none where it is not offered - the
    artifacts' writer where no storage of content is configured, the workspace's reader where the
    work may use none. No method of a face takes a context or an identity, and what it hands out is
    data, not an object of the kernel - but for a handle, named as one, that carries a right beyond
    the call: the workspace's terminal and shell.
*   **Asked for where it is used, never kept.** A face acts in the context of the call that asked
    for it - the rights as they are at each call -, so a component asks for it again each time.
*   **Declared.** A component kind declares the faces it reads and writes by
    (`ComponentKind::reads`, `writes`); a kernel in which a declared face is bound by no plugin is
    not built. The declaration guards the build alone: a face is handed to whichever component asks
    for it, as the situation and the turn know no kinds.
*   **Named alike.** A reader is a `…Reader`, a writer a `…Writer`, unless what it does names it
    better (`ArtifactPublisher`).

## What a component kind outside the kernel may use

A component kind in a crate of its own uses the kernel through what a component of the kernel uses
too, and nothing else - there is no prelude that re-exports at large, so that it shows which types a
plugin or a library depends on:

*   **To register it:** `LibraryComponents::register` or `register_async` for a library's kind
    (`ComponentRegistry::register` for a plugin's), `ComponentKind` with the parameters it takes
    (`params`, a struct that implements `Params`), the facets it `provides`, those of others it
    `needs`, the services it `uses` and the extensions it `reads` and `writes` by,
    `ComponentFacets`, and `config_of_params` to read its `params`. A kind that declares no
    parameters takes none: every one it is given is refused
    ([COMPONENTS.md](COMPONENTS.md#parameters)).
*   **While it is built:** the `ComponentContext` - which holds no name: the graph keeps the one it
    is built in (issue 516) -, `service::<T>()`, the services of the kernel it declared, and `all`
    and `one`, the facets of others it declared it needs. What a container accepts follows from
    these declarations ([COMPONENTS.md](COMPONENTS.md#what-a-container-accepts)). A service that
    decides for the kernel - a policy, one of the kernel's managers, a store - is the kernel's, and
    a component of a library does not declare it - the kernel is not built where one does: what it
    needs decided it asks of the kernel, through the `Turn` and the `Situation` it is called with
    (`may_reclassify`, `may_copy_information`, `copy_attachment`; the executor's `admit_tool_call`,
    issue 516), and what it reads it reads through the readers (`Situation::session`,
    `SessionReader`). What it needs of another crate it declares as the trait that crate offers for
    it, or, of a system service, as the extension it reads or writes by: a reader its situation
    hands out (`Situation::reader`), a writer its turn (`Turn::writer`).
*   **The facets it offers**, with what their signatures name: for a tool provider `ToolProvider`,
    `ToolOutput`, `ToolError` and `ToolResult`, `ApprovalMode`, and the `Turn` a call is made in;
    for an external agent `ExternalAgent`, `OutboundMessage` and `Said`, `ExternalEvent`,
    `ExternalStream`, `ExternalAgentError` and `ExternalAgentResult`.
*   **If it talks to a remote system:** `caelumora_api::remote`, which describes its errors
    as the kernel's are described - `describe_request_error`, `describe_chain` (issue 326) -, and
    the `Secrets` its credential is read of (`RemoteEndpoint::bearer_token`): the work's, from its
    `Situation` (`secrets`), or handed to an `ExternalAgent` with every call (issue 516).
*   **The model's types** of what it handles: `Tool`, `RemoteEndpoint`, `Classification`, ...

What a plugin needs beyond that is made visible when a plugin needs it, as part of the kernel's API,
rather than reached around (issue 325).

## Errors: a plugin answers the kernel in the kernel's types

Where the kernel calls a plugin - a facet of its component, a `ScopeMember` - the plugin answers
with an error type of the kernel, never with one of its own inside an `anyhow` (issue 336). The
kernel decides what a person is told of an error by the types it knows -
`caelumora_kernel::error::user_message` for the user of a command and the TUI, and
`From<anyhow::Error> for ApiError` for the REST API -, and a plugin's types are not among them:
inside an `anyhow`, whatever a plugin's error says is shown as a failure that points to the log. The
agents' library, which defines the facets most plugins serve, is a judge of its own in this: what an
agent's owner is told when an answer fails is judged by `caelumora_agents::error::user_message`,
which knows the error types of those facets and asks the API's judgement for the rest
(`caelumora_api::error::user_message_with`, issues 370, 516). A registry of judgements the plugins
add is not needed, and would be the registry of downcasts rejected below. A library may define a
facet in turn that other libraries serve - `caelumora-compaction` the `KnowledgeRestorer`, which the
`skills` component serves (issue 464) -: they depend on it and answer in its error type,
`CompactionError`, as they answer the agents in theirs; what is shown of it, the defining library
decides where it answers the agents - the `compaction` component carries its text into its
`ActivityError` and `CommandError`.

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

The crate's own error type is part of its API (`SkillError`, `ArtifactError`, issue 516): its
manager, its views and its routes answer in it - the routes' handlers by a `From<…> for ApiError` of
the plugin's (see [REST-SERVER.md](REST-SERVER.md#errors)) -, and it reaches the library through the
faces, whose component translates it as above; it reaches the kernel no further. `Plugin::register`
and `SeedSection::seed` answer `anyhow`: they run at startup, and what fails there ends the program
and goes to the log.

What is not done instead, and why: a type of the kernel carried as `#[source]` that marks a message
as safe to show, after the pattern of `Refused` - but a refusal is what caused the error carrying
it, and a mark of what may be shown is no cause -; and a trait such as `UserFacing` the kernel finds
in the chain - a `&dyn Error` cannot be asked for a trait of its type, so it would take a registry
of downcasts beside the chain.
