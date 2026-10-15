# Coding Guidelines

The general rules for code in Caelumora: style, error handling, asynchronous code, logging and
tests. Rules that belong to one subject live with it - where permission checks go in
[SECURITY.md](SECURITY.md), how a component is written in [COMPONENTS.md](COMPONENTS.md).
[ARCHITECTURE.md](ARCHITECTURE.md) gives the overall picture.

## Comments and documentation

*   **Doc comments:** use `///` for public APIs (structs, traits, methods). Document parameters,
    return values and potential errors.
*   **Internal comments:** use `//` for implementation details. Explain *why* something is done if
    it is not obvious from the code.
*   **Module docs:** use `//!` at the top of new modules to describe their purpose.
*   **Consistency:** mirror the comment frequency and style of the module you are editing.
*   **Comments wrap at 100 characters**, as the Markdown documents do (`AGENTS.md`). Older
    comments are wrapped narrower; they are rewrapped when they are changed anyway, not on their
    own.
*   **No line of prose starts with a dash.** In a doc comment, a line that begins with `- ` is a
    list item to rustdoc, and the sentence it continues breaks apart (clippy:
    `doc_lazy_continuation`). Wrap so that a dash ends the line before it instead.
*   **No clippy warnings.** `cargo clippy --workspace --all-targets` stays silent; the pre-commit
    hook in `.githooks` refuses a commit of Rust code otherwise, once a clone has it enabled
    (`git config core.hooksPath .githooks`). Where a lint is wrong for a case, say why with
    `#[expect(clippy::…, reason = "…")]` on the item - `expect` rather than `allow`, so that it is
    noticed once the lint no longer fires.

### What goes where

What is known about the code has three places, each kept by whoever changes what it describes:

*   **The documents in `docs/`** explain what the code does not show by itself: the concepts and
    who owns them; the principles and rules - invariants, orders, who checks what, what is never
    done and why; and how the parts fit together. A type or method is named where a reader needs
    the name to find the code - the name, not what it does in detail. A document says what holds,
    not how it came to be, and is read before every piece of work on its subject, so every line of
    it costs every reader.
*   **Doc comments and module docs** (`///`, `//!`) hold what one type, method or module does in
    detail: its variants and fields, its steps and their order, its errors, its constants. They are
    changed and reviewed with the code they describe.
*   **Issues** hold how something came to be: the options weighed and why one was chosen. A
    document links an issue only where a rule would otherwise look wrong or arbitrary and the issue
    says why - not to record which issue added what.

A change updates the document on its subject where it changes a concept, a rule or how the parts
fit together, and only the doc comments where it changes how something is done. A rule that only a
document states is moved, not dropped, when the document is shortened.

## Imports

A type, trait or function used by name is imported with `use` at the top of the module and written
by its name, not by its path (`impl Scoped for Agent`, not `impl crate::scope::Scoped for
Agent`) - also when adding the import means touching the import block for a single use. A path is
written out only where an import cannot serve: inside a `macro_rules!`, which expands in other
modules and writes `$crate::…`; where two items of one name meet (`crate::chain::Stored` and
`crate::store::Stored`), unless one of them is renamed on import; and where the module around it
calls something by its module on purpose, so that the call says where the function lives.

## Crates and interfaces

*   **Model and runtime.** When you change `caelumora-model`, keep `caelumora-runtime` compatible
    with it.
*   **REST and web UI.** When you change the REST API - its routes in `caelumora-rest-server`, its
    types in `caelumora-rest-api` -, keep `caelumora-web-ui` compatible with it.
*   **Components.** A new component kind is registered in the `ComponentRegistry`; where it goes and
    how it is named is described in [COMPONENTS.md](COMPONENTS.md).

### A module's manager

A manager is the one owner of the lifecycle of a kind - it makes, deletes, restores and purges
it: `AgentManager`, `SessionManager`, `WorkspaceManager`, `GroupManager`, `ControllerManager`,
`UserManager`. It lives in its module's `manager.rs`, declared `mod manager;` and exported from
`mod.rs` with `pub use manager::XyzManager;`, so that it is imported by its module's path
(`crate::workspace::WorkspaceManager`). `mod.rs` holds the module's description, its error type,
what its parts share, and its exports, so that the manager's methods and tests do not bury them.
Test helpers the module offers to other modules' tests are its `testing.rs` (issue 364).

### Small return values

A function that returns two values, called from a few places close to it - inside its module or
crate, not a public API - returns a tuple, not a type made for it: `Turn::full_context` returns
`(messages, tools)`, and the caller names the parts where it destructures them. A type that only
bundles a return value is one more name to read and keep. A type is worth it once the value has a
meaning of its own: it gets methods, travels further than its caller, is stored or serialized, has
more than two parts, or its parts have the same type and could be swapped unnoticed.

### Runtimes and their entities

A running object - `SessionLoop`, `ChannelLoop`, `ActorRuntime` - and the stored entity it holds
(`Session`, `Channel`) share their work by three rules:

*   **The model derives.** A value derived from an entity is computed in `caelumora-model` - a
    method of the entity (`Agent::security_context`) or a constructor of the derived type
    (`SessionPrivacy::of(&Session)`) - never in the runtime, so that the stored document and the
    running object cannot disagree about it.
*   **A runtime answers about its own entity, not about the one above.** A loop may have getters for
    attributes of its own entity that callers need often; they return owned values, so that no
    `arc_swap::Guard` is held across an `await`. Everything else is read from its snapshot. The
    entity above is reached through the parent - `runtime.channel_loop()`,
    `channel.session_loop()` - never through a forwarder such as a `channel_classification()` on the
    actor's runtime. What holds when the parent is missing is the caller's to decide.
*   **One name, one kind of thing.** `channel()` and `session()` return the entity's document as it
    is now; `channel_loop()` and `session_loop()` the running object above; loading any stored
    entity is `load_…` (`ActorRuntime::load_session`).

### Services are held, not fetched

A type takes the services it works with - the `AccessPolicy`, the stores, the event bus, the
`ComponentRegistry` - when it is made, and keeps them as fields:

*   **Constructor injection.** `new` takes the services, or takes them once from the runtime it is
    given, and stores them as `Arc<dyn Trait>`: `dyn`, since the policy and the stores are chosen
    at run time, and `Arc`, since the type lives in a task. No service is fetched again through the
    runtime at every use, and none sits in a global (`static`, `OnceLock`), which would hide the
    dependency and tie every test to one instance. The services never change while the process
    runs, so a held one cannot go stale.
*   **What it holds says what it needs.** A type holds the services it uses, not the runtime that
    reaches all of them. Several services that go together travel as a small context holding
    exactly those - `ComponentContext`, `Views` - never as one that holds everything again. A
    scoped runtime such as `ActorRuntime` is such a context; it does not hand out the
    `RuntimeManager` behind it, so that its users cannot reach past it (issue 359).
*   **No threading next to the owner.** A service is not passed into a function beside the object
    that already holds it; the function becomes a method of that object, or takes the object.
*   **Accessors lend.** An accessor returns `&Arc<dyn …>` or `&dyn …`; whoever keeps the service
    clones it, with `Arc::clone` (see below).
*   **One step to ask.** A check reads as the question it is -
    `views.access_policy().may_approve(views.context(), …)` - not as a walk through the runtime. A
    type that is asked often in someone's name holds that name with the services, as `Views` holds
    its viewer.

### Cloning shared pointers

An `Arc` (or `Rc`) is cloned with `Arc::clone(&value)`, not `value.clone()`: the call says that
only a reference count goes up, where `.clone()` reads the same as a deep copy of a `Vec` or a
`String` - the convention of the Rust book, and what clippy's `clone_on_ref_ptr` asks for, which the
workspace turns on (`[workspace.lints.clippy]` in `Cargo.toml`). Where an
accessor lends `&Arc<…>`, it is `Arc::clone(runtime.access_policy())`. A clone is made only
where an owner is needed - a field, a task, a value moved elsewhere; a call that only uses the
value borrows it. An `Arc<Concrete>` that becomes an `Arc<dyn Trait>` where it is passed names its
type, `Arc::<MemoryStore>::clone(&store)`, since the type cannot be inferred from the place it goes.

## Error handling

Errors carry what failed, and only what may be shown goes to users.

### Types

*   Use `anyhow::Result` for general application logic, especially in the runtime, where diverse
    error types are aggregated: code that gathers errors and passes them on, within an operation
    or at the top of a task - not at the boundary to implementations of others.
*   Use `thiserror` for domain-specific error types, as in `caelumora-model`.
*   A method of a trait answers a specific error type, never `anyhow`: who implements it is not
    fixed - a module of the core today, a plugin tomorrow -, and its caller has to tell its errors
    apart. The type is the contract: what the caller can distinguish - what may be shown, a
    refusal, a store's error -, and the compiler makes every implementation translate into it.
    Context that `anyhow` attaches with `.context(…)` becomes a variant of the type
    (`ContextError::Component`). For a plugin, the type is one of the core's (see
    [PLUGINS.md](PLUGINS.md#errors-a-plugin-answers-the-core-in-the-cores-types)). The exceptions
    are named, so that "its caller only logs it" is no excuse for the next one: traits whose only
    caller is the top of the program or of a task and logs the error whole - `Plugin::register`,
    `SeedSection::seed`, `Actor::run` and `Actor::catch_up`.
*   Avoid `unwrap()` and `expect()` in production code. Use `?` or handle the error explicitly.

### Messages name what failed

*   An error caused by configuration names the component id (and kind) and the parameter.
    `ComponentError::Build` wraps every failure of a component graph with the component's id and
    kind.
*   An error caused by a remote system names the endpoint, without credentials, query or fragment
    (`caelumora_model::remote::redact_endpoint`), and the chain of its causes (the runtime's
    `remote::describe_chain`; for a failed request without the URL `reqwest` writes into it,
    `describe_request_error`). `LLMError`
    names the endpoint, the model and the backend's own explanation (`Connection`, `Rejected`,
    `Backend`, `InvalidResponse`).
*   Messages never contain secret values or file paths, because they may be shown to users.

### What users see

What users may see is decided in one place, `caelumora_runtime::error::user_message`. It looks
through an error chain for a domain error written to be read and returns its message. Anything else
stays in the log.

A new error type of the core meant for users must meet the rules above and be added to
`user_message`. A plugin's error type is never added there: where the plugin answers the core, it
answers in a type of the core's (see
[PLUGINS.md](PLUGINS.md#errors-a-plugin-answers-the-core-in-the-cores-types)).

### Errors in a turn end the turn, not the agent

If an error occurs while an `AgentActor` is in a turn - an unreachable model, a fail-closed
component, a failing tool executor - the agent logs it, discards the turn's unfinished work, emits
`ActorEvent::TurnGivenUp` with `GivenUpReason::Failed { message }` and returns to `Idle`. Don't
let a turn-time error escape `AgentActor::run`.

Moderators forward `TurnGivenUp` to the users, whose actors pass it on to the UI. The failure's
message is not given to a model: what it is told of a turn given up is a fixed text of the
runtime's, with the reason. What a model is told of a failed tool call is the `ToolExecutor`'s (see
[TOOLS.md](TOOLS.md#results)).

### REST errors

REST handlers return `Result<_, crate::error::ApiError>`. Never map an error to a bare status code;
the server answers every error with problem details, as described in
[REST-SERVER.md](REST-SERVER.md#errors).

*   Pass an error on with `?`: it answers itself. An error type that reaches a handler implements
    `From<…> for ApiError`, answering each of its variants with a status and a code; a new variant
    gets its answer there, not in the handlers.
*   An error that means a refusal carries the `Refused` that `Enforcement::require` hands over as
    its `#[source]` - never `#[error(transparent)]` directly around it, which would hide it from
    the chain.
*   Use `ApiError::internal` for server errors: it logs the full chain and discloses only what
    `user_message` allows.
*   Use the `Json` and `Path` extractors from `crate::error`, so that malformed requests get problem
    details too.

## Asynchronous programming

*   Caelumora makes heavy use of `async`/`await`.
*   Prefer `tokio` for async primitives (`Mutex`, `RwLock`, `mpsc`, ...).
*   Traits with async methods use `#[async_trait]` if they need to be object-safe, or if the
    surrounding code already uses it. Many internal traits use `BoxStream` for streaming responses.

## Logging

### Setup

*   Every crate logs through `tracing` (`tracing::{debug, info, warn, error, trace}`), never through
    `log` or `println!`. Records from dependencies that still use `log` are forwarded into
    `tracing`.
*   Libraries never install a subscriber. The programs install it with `caelumora_logging::init`;
    `caelumora-logging` is the one place of the setup and the only crate that depends on
    `tracing-subscriber` (issue 330).
*   `RUST_LOG` selects the records; the configuration's `logging` section sets the format (`text`,
    the default, or `json`) and an optional file (see [CONFIGURATION.md](CONFIGURATION.md)). The
    REST server logs to stderr, the TUI to `local/tui.log`, so that log lines do not break its
    prompt.

### Levels

*   `error!` only for failures that abort an operation.
*   `warn!` for degraded but continuing operation: an MCP server that failed discovery, a queue that
    dropped an event.
*   `info!` for lifecycle transitions: a channel or actor starting or stopping, a session created.
*   `debug!` for per-event detail. Anything that happens once per message or per store operation is
    `debug!` at most; once per streamed chunk is `trace!`.

### Content

*   Message text, prompts and tool arguments only at `trace!`.
*   At `debug!` and above, log ids, event kinds (`ActorEvent::kind`, `Envelope::describe`) and
    sizes.
*   Credentials, tokens and cookies are never logged, at any level.

### Correlation

A message crosses several tasks - REST handler, `UserActor`, `ChannelLoop`, moderator,
`AgentActor` - connected by channels, so `tracing`'s span nesting alone cannot follow it. The ids of
its `EventContext` can. Ids therefore belong on spans, not repeated in messages.

Every span that works on an envelope carries the ids as fields named after the `EventContext`
fields: `span_id`, `event_id`, `correlation_id` and `span_depth`. There are three kinds of span:

*   **`request`** - one REST request, with `method`, `uri`, `request_id`, the authenticated
    `user_id`, and the `span_id` of the envelopes the request started.
*   **`envelope`** - handling one envelope in the `ChannelLoop` or an idle agent
    (`caelumora_runtime::logging::envelope_span`), with session, channel, sender and event kind.
*   **`turn`** - one iteration of an agent's cognitive cycle, with agent and state and the ids of
    the turn's `EventContext`. An idle or sleeping agent runs in an `agent` span instead.

Searching the log for one `span_id` therefore finds the request, the routing and the agent's turn
that answered it.

## Testing

### Unit tests

*   **Placement:** in a `mod tests` block at the bottom of the file, guarded by `#[cfg(test)]`.
*   **Shared fixtures:** what the tests of several files use lives in a `#[cfg(test)] pub(crate) mod
    testing` of the module it builds, e.g. `session::testing`. The tests of the actors in a running
    channel share the channel's fixtures and live with them in `channel/tests/`.
*   **Async tests:** use `#[tokio::test]` for tests involving async code.
*   **Naming:** use descriptive names, e.g.
    `an_answer_is_its_whole_messages_and_what_was_being_written`.
*   **Assertions:** use `assert!`, `assert_eq!` and `expect()` with descriptive messages.

### Integration tests

Integration tests go into the `tests/` directory of the relevant crate, or into `examples/` if they
serve as documentation.
