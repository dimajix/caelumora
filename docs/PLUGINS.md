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
plugin that brings a component kind alone, such as `caelumora-mcp` (issue 325), `caelumora-a2a`
(issue 326) or `caelumora-ollama` (issue 329), implements `Plugin` only and registers the kind in
`register`; whether a kind of a facet of which there is one is its default, the program says, not
the plugin (`ComponentRegistry::set_default_kind`), so that two plugins of a facet cannot both
claim it; one that brings
backends alone, such as `caelumora-open-terminal` (issue 327), registers them in `backends` and
nothing in `register`. A plugin's tests use
the world of the security suite - the runtime with the recording ownership policy and three users -,
`caelumora_runtime::testing`, as a dev-dependency with the feature `test-support`.

## What a component kind outside the core may use

A component kind in a crate of its own uses the runtime through what a component of the core uses
too, and nothing else - there is no prelude that re-exports at large, so that it shows which types
a plugin depends on:

*   **To register it:** `ComponentRegistry::register` or `register_async`, `ComponentKind` with the
    facets it `provides`, those of others it `needs` and the services it `uses`,
    `ComponentFacets`, and `config_of_params` to read its `params`.
*   **While it is built:** the `ComponentContext` - `security_context`, in whose name it is built,
    `service::<T>()`, the services of the runtime it declared, and `all` and `one`, the facets of
    others it declared it needs. What a container accepts follows from these declarations
    ([COMPONENTS.md](COMPONENTS.md#what-a-container-accepts)).
*   **The facets it offers**, with what their signatures name: for a tool provider `ToolProvider`,
    `ToolOutput`, `ToolError` and `ToolResult`, `ApprovalMode`, and the `Turn` a call is made in;
    for an external agent `ExternalAgent`, `OutboundMessage` and `Said`, `ExternalEvent`,
    `ExternalStream`, `ExternalAgentError` and `ExternalAgentResult`.
*   **If it talks to a remote system:** `caelumora_runtime::remote`, which describes its errors
    as the core's are described - `describe_request_error`, `describe_chain` (issue 326).
*   **The model's types** of what it handles: `Tool`, `RemoteEndpoint`, `SecretManager`,
    `Classification`, ...

What a plugin needs beyond that is made visible when a plugin needs it, as part of the runtime's
API, rather than reached around (issue 325).

## Errors: a plugin answers the core in the core's types

Where the core calls a plugin - a facet of its component, a `ScopeMember` - the plugin answers with
an error type of the core, never with one of its own inside an `anyhow` (issue 336). The core
decides what a person is told of an error by the types it knows -
`caelumora_runtime::error::user_message` for an agent's owner, the user of a command and the TUI,
and `From<anyhow::Error> for ApiError` for the REST API -, and a plugin's types are not among them:
inside an `anyhow`, whatever a plugin's error says is shown as a failure that points to the log.

So the trait names its error type, and the plugin translates its own error into it where it
answers - deciding there what may be shown, what is a refusal and what is a failure:

*   a tool provider answers `ToolError`, an agent's command `CommandError`, a knowledge restorer
    `CompactionError`, an external agent `ExternalAgentError`;
*   a prompt provider and a context provider answer `context::ContextError`, a message processor
    `message::ProcessingError` - each with `Failed`, a failure in words that may be shown, and the
    view's and the store's errors, judged as those are;
*   a `ScopeMember` answers `StoreResult`; what lies behind an entity - a workspace's place, an
    artifact's content - is its storage too, and a failure to remove it a `StoreError::Backend`.

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
