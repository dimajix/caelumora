# Coding Guidelines

The general rules for code in Caelumora: style, error handling, asynchronous code, logging and
tests. Rules that belong to one subject live with it - where permission checks go in
[SECURITY.md](SECURITY.md), how a component is written in [COMPONENTS.md](COMPONENTS.md).
[ARCHITECTURE.md](ARCHITECTURE.md) gives the overall picture, the principles of design are in
[AGENTS.md](../AGENTS.md#design-principles).

## Rules for every change

The rules most often missed, each with the section that gives its reasons. Every change is checked
against this list, rule by rule, before it is reported (the self-review of
[AGENTS.md](../AGENTS.md#workflow)).

*   Import what you use with `use` at the top of the module and write it by its name: `impl
    Stored for Skill`, `Skill::KIND` - never `impl crate::store::Stored for Skill` or
    `caelumora_model::skill::Skill::KIND`, also not for a single use or in a test. A function may
    be called by its module (`time::sleep` after `use tokio::time;`), never by a longer path. A
    path is written out only where an import cannot serve ([Imports](#imports)).
*   Every module begins with `//!` whose first sentence names its responsibility
    ([Comments and documentation](#comments-and-documentation)).
*   No `unwrap()` or `expect()` in production code ([Types](#types); clippy checks it).
*   A method of a trait answers a specific error type, never `anyhow` - except the few named in
    [Types](#types).
*   Errors in a turn end the turn, not the agent: don't let a turn-time error escape
    `AgentActor::run` ([Errors in a turn](#errors-in-a-turn-end-the-turn-not-the-agent)).
*   Error messages name what failed and never contain secret values or file paths
    ([Messages name what failed](#messages-name-what-failed)).
*   Log through `tracing`, never through `log` or `println!`; message content only at `trace!`,
    credentials never ([Logging](#logging); clippy checks `println!`).
*   REST handlers return `Result<_, crate::error::ApiError>`; never map an error to a bare status
    code ([REST errors](#rest-errors)). Keep the REST API compatible with `caelumora-web-ui`
    ([Crates and interfaces](#crates-and-interfaces)).
*   A type takes the services it uses - the `AccessPolicy`, the stores, the event bus - when it
    is made and keeps them as fields; it does not fetch them through the runtime at every use
    ([Services are held, not fetched](#services-are-held-not-fetched)).
*   A pure access question is a method of `AccessQuestions`, an operation checks its own permission
    with `Enforcement::require` - before it asks anything of others - and a frontend never asks the
    `AccessPolicy` itself (see [SECURITY.md](SECURITY.md)). A plugin does the same through
    extension traits of its own - its questions on `AccessQuestions`, its views on `Views` - and
    adds nothing about its kind to the core (see [PLUGINS.md](PLUGINS.md)).
*   A new action that acts in someone's name, or a change to what an action asks, comes with its
    test in the security suite (`caelumora-runtime/tests/security_suite.rs`, an agent's in
    `caelumora-agents/tests/security_suite.rs`, see
    [SECURITY.md](SECURITY.md#the-security-suite)) - a plugin's in its own tests, in the suite's
    world (`caelumora_runtime::testing`, feature `test-support`; with agents,
    `caelumora_agents::testing`).
*   A change to what is stored comes without compatibility code; the summary says which stored
    data has to be deleted ([Crates and interfaces](#crates-and-interfaces)).
*   Comments and Markdown documents wrap prose at 100 characters; `scripts/line-length.sh` finds
    the lines of a change that are longer
    ([Comments and documentation](#comments-and-documentation)).
*   `cargo clippy --workspace --all-targets` stays silent
    ([Comments and documentation](#comments-and-documentation)).

## Comments and documentation

*   **Doc comments:** use `///` for public APIs (structs, traits, methods). Document parameters,
    return values and potential errors.
*   **Internal comments:** use `//` for implementation details. Explain *why* something is done if
    it is not obvious from the code.
*   **Module docs:** every module begins with `//!`, and its first sentence names the module's
    responsibility - one sentence, without an "and also" (the test of
    [one owner per concern](../AGENTS.md#design-principles)). What follows describes the module in
    detail. A change after which that sentence no longer holds is a
    [finding](../AGENTS.md#findings); a module without such a sentence gets it when its content
    is changed anyway. A mechanical change across many modules - a lint's fix, a rename - does not
    count, so that it stays one change of its own.
*   **Consistency:** mirror the comment frequency and style of the module you are editing.
*   **Comments wrap at 100 characters**, as the Markdown documents do. Older comments are wrapped
    narrower; they are rewrapped when they are changed anyway, not on their own.
*   **Markdown documents** (`docs/`, `issues/`, `AGENTS.md`, `.claude/agents/`) wrap prose at 100
    characters. Headings, table rows, code blocks and links that would not fit stay on one line;
    field lines such as `**Status:**` keep a line of their own.
*   **Line lengths are counted by `scripts/line-length.sh`**, in characters, not bytes, by the
    exceptions above. Without arguments it checks the lines a change adds or changes, against
    `HEAD`, and new files whole; given files, it checks them whole. Lengths are not counted by hand
    or with another tool, which counts bytes or knows no exceptions.
*   **No line of prose starts with a dash.** In a doc comment, a line that begins with `- ` is a
    list item to rustdoc, and the sentence it continues breaks apart (clippy:
    `doc_lazy_continuation`). Wrap so that a dash ends the line before it instead.
*   **No clippy warnings.** `cargo clippy --workspace --all-targets` stays silent; the pre-commit
    hook in `.githooks` refuses a commit of Rust code otherwise, once a clone has it enabled
    (`git config core.hooksPath .githooks`). Where a lint is wrong for a case, say why with
    `#[expect(clippy::…, reason = "…")]` on the item - `expect` rather than `allow`, so that it is
    noticed once the lint no longer fires.
*   **Rules a lint checks.** `[workspace.lints]` in `Cargo.toml` turns on the lints that check
    rules (issue 452): `unwrap_used` and `expect_used` ([Types](#types)), `print_stdout` and
    `print_stderr` ([Logging](#logging)), `allow_attributes` and
    `allow_attributes_without_reason` (the rule above), `unreachable_pub` (visible only what is
    used from outside, [AGENTS.md](../AGENTS.md#design-principles)); and `dbg_macro` and `todo`,
    so that no debugging output and no unfinished code is committed. `clippy.toml` allows
    unwrapping and printing in tests; test code outside `#[cfg(test)]` - a `testing` module, a
    suite, the helpers of an integration test - says so at its top with
    `#![expect(clippy::unwrap_used, …, reason = "test code: …")]`. `unreachable_pub` checks only
    part of its rule: it finds a `pub` item its crate's root cannot reach, not one in a public
    module that only its own crate uses, nor a `pub(crate)` item that only its own module uses.

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
calls a function by its module on purpose, so that the call says where the function lives - then
the module is imported and named in the call (`use tokio::time;`, then `time::sleep(…)`), never a
longer path (`tokio::time::sleep(…)`). The
derive of `thiserror` is written by its path, `#[derive(Debug, thiserror::Error)]`: imported,
`Error` would stand beside `std::error::Error` and `anyhow::Error`, which many of the same
modules use.

## Crates and interfaces

*   **Model and runtime.** When you change `caelumora-model`, keep `caelumora-runtime` compatible
    with it.
*   **REST and web UI.** When you change the REST API - its routes in `caelumora-rest-server`, its
    types in `caelumora-rest-api` -, keep `caelumora-web-ui` compatible with it.
*   **Components.** A new component kind is registered in the `ComponentRegistry`; where it goes and
    how it is named is described in [COMPONENTS.md](COMPONENTS.md).
*   **A new crate.** A crate of the workspace takes `version`, `edition` and `license` from
    `[workspace.package]` in the root `Cargo.toml` (`version.workspace = true`, ...) and its lints
    from `[workspace.lints]` (`[lints] workspace = true`); it states none of them itself. Without
    them it builds as well, but has no license and escapes the lints.
*   **Dependencies.** Every dependency - of another crate of the workspace too - is declared once
    in `[workspace.dependencies]`, with its version, `default-features = false` if any crate needs
    it, and the features every crate using it needs. A crate names it with `workspace = true` and
    adds only its own features and `optional`. A version is raised there, for all crates at once.
*   **Stored data.** Stored data need not stay readable: until further notice, old data may be
    deleted. A change to what is stored comes without compatibility code - no `serde` aliases, no
    readers of an older form. Instead, the summary of the change says which stored data has to be
    deleted.

### A module's manager

A manager is the one owner of the lifecycle of a kind - it makes, deletes, restores and purges
it: `AgentManager`, `SessionManager`, `WorkspaceManager`, `GroupManager`, `ControllerManager`,
`UserManager`. It lives in its module's `manager.rs`, declared `mod manager;` and exported from
`mod.rs` with `pub use manager::XyzManager;`, so that it is imported by its module's path
(`crate::workspace::WorkspaceManager`). `mod.rs` holds the module's description, its error type,
what its parts share, and its exports, so that the manager's methods and tests do not bury them.
Test helpers the module offers to other modules' tests are its `testing.rs` (issue 364).

Its methods are named by the verbs of the lifecycle, without the kind - the manager names it:
`create`, `delete`, `restore` and `purge`, and for what runs, `open` and `stop`
(`SessionManager::open`, `AgentManager::delete`; issue 354).

### Small return values

A function that returns two values, called from a few places close to it - inside its module or
crate, not a public API - returns a tuple, not a type made for it: `Turn::full_context` returns
`(messages, tools)`, and the caller names the parts where it destructures them. A type that only
bundles a return value is one more name to read and keep. A type is worth it once the value has a
meaning of its own: it gets methods, travels further than its caller, is stored or serialized, has
more than two parts, or its parts have the same type and could be swapped unnoticed.

### States are types

Fields that make sense only in certain combinations - two `bool`s of which only three combinations
occur, an `Option` that is set exactly when another field has a certain value - become an `enum`
whose variants hold what belongs to them. A state that cannot be represented needs no check at its
callers, and a `match` over it is checked by the compiler. The same holds for parameters: a `bool`
that switches a function between two behaviours becomes two functions, or an `enum` that names the
behaviours.

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
    entity above is reached through the parent - `channel.session_loop()` - never through a
    forwarder such as a `session_privacy()` on the channel's loop. What holds when the parent is
    missing is the caller's to decide.
*   **The `ActorRuntime` is the actor's interface.** The one exception to the rule above: an actor
    reaches its channel through its `ActorRuntime`. Its classification and attributes are read from
    the channel's controller, which the runtime holds as the session's (issue 374); what only the
    running channel can answer - its users for the questions of a tool call, its workspace (issue
    370) - is asked of the channel, and answered `None` or refused while the channel is being built
    or once it is gone, so that the caller still decides what that means. The public API of
    `ChannelLoop` is the frontends'; it acts in a user's name too (`ChannelLoop::user_actor`), and
    is not an actor's. What an actor needs of its channel and does not find on its `ActorRuntime` is
    added there. `ActorRuntime::channel_loop` stays the runtime's (`pub(crate)`): for the user's and
    the approver's actors, which act for a person as the frontends do, and for the views.
*   **One name, one kind of thing.** `channel()` and `session()` return the entity's document as it
    is now; `channel_loop()` and `session_loop()` the running object above; loading any stored
    entity is `load_…` (`ActorRuntime::load_session`).
*   **Found by id, by name, for someone.** What a running object holds is found by its id under
    the plain name of what it is (`SessionLoop::channel(id)`, `ChannelLoop::actor(id)`), by its name
    with `_named` (`SessionLoop::channel_named`, `ChannelLoop::agents_named`), and in someone's name
    - who may reach it, asked - with `_for` (`SessionLoop::channel_for(context, id)`); never
    `_by_id` or `_by_name` (issue 354).

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
    `SeedSection::seed` and `Actor::run`.
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

What users may see is decided by whoever defines an error type, in one function each: the runtime's
types in `caelumora_runtime::error::user_message`, the types of the agents' logic - the errors of
its facets - in `caelumora_agents::error::user_message` (issue 370). Each looks through an error
chain for a domain error written to be read and returns its message; the agents' asks its own
judgement first and the runtime's for the rest, in one walk (`error::user_message_with`), so that
the outermost error that may be shown is shown. Anything else stays in the log.

A new error type meant for users must meet the rules above and be added to the function of whoever
defines it. A plugin's error type is never added there: where the plugin answers the core or the
agents' logic, it answers in a type of theirs (see
[PLUGINS.md](PLUGINS.md#errors-a-plugin-answers-the-core-in-the-cores-types)).

### Rolling back

An operation that stored something and then fails part of the way is rolled back: what it stored
is set back to how it was before - in the runtime's name, since the operation was permitted -, and
its caller gets the error. A step that does so is named `roll_back_…` after what it rolls back
(`SessionManager::roll_back_creation`, `SessionLoop::roll_back_channel_deletion`). It is no
transaction: what cannot be set back is logged and stays (issue 354).

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
*   Use the `Json`, `Path` and `Query` extractors from `crate::error`, so that malformed requests
    get problem details too.

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

A test uses what the code offers its callers. One that needs an internal - a field or method made
visible for it, beyond what the `testing` modules below offer - says that a cut is wrong: it is a
[finding](../AGENTS.md#findings), and the visibility is not widened for it.

### Unit tests

*   **Placement:** in a `mod tests` block at the bottom of the file, guarded by `#[cfg(test)]`.
*   **Shared fixtures:** what the tests of several files use lives in a `testing` module of the
    module it builds, e.g. `session::testing` - `#[cfg(test)] pub(crate)` while only the crate's own
    tests use it, `#[cfg(any(test, feature = "test-support"))] pub` once the tests of another crate
    do (`channel::testing`, `session::testing`, `workspace::testing`, `component::testing`, issue
    370). What such a module adds to a type for tests - `ChannelLoop::actor_of` - is named apart
    from the type's own API and widens nothing outside `test-support`. The tests of the actors in a
    running channel live in `channel/tests/` of the runtime, those of the agents in
    `channel_tests/` of `caelumora-agents`.
*   **A runtime in tests:** a test builds its runtime with `testing::TestRuntime` (issue 375), whose
    parts all have defaults - a new memory store, no policy, the default rules, the default
    configuration, the runtime's component kinds and agents that answer "ok" - and names only those
    it differs in: `TestRuntime { plugins: vec![&AgentsPlugin], ..Default::default() }.build()`,
    or `try_build()` for a test of what refuses to be built. Nobody else calls `RuntimeManager::new`
    in a test, so that a change to it is a change to one place, and what a test differs in is what
    it names. `session::testing::test_runtime()` and `owned_runtime(rules)` are the two runtimes
    most tests share; the program's own test (`caelumora-app`) builds the runtime as the program
    does.
*   **Agents in the runtime's tests:** a test of what a channel, a session or the views do with an
    agent among their participants runs a scripted agent (`testing::ScriptedAgents`, issue 370),
    which begins, ends or hangs in its answers as told; a test of what an agent does runs the
    agents' plugin, in `caelumora-agents`. A test that puts an actor straight into a channel or
    before a moderator - whom it finds, wakes or cancels, whoever the actor is - puts a
    `testing::StandIn` there, which does nothing (issue 372). No other test implements `Actor`,
    except one that needs what no actor of the runtime can do, such as sending envelopes in
    another's name - and then names the actor so.
*   **Async tests:** use `#[tokio::test]` for tests involving async code.
*   **Naming:** use descriptive names, e.g.
    `an_answer_is_its_whole_messages_and_what_was_being_written`.
*   **Assertions:** use `assert!`, `assert_eq!` and `expect()` with descriptive messages.

### Integration tests

Integration tests go into the `tests/` directory of the relevant crate, or into `examples/` if they
serve as documentation.
