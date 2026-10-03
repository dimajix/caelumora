# 210 — The A2A agent is one file holding protocol, translation, transport and component

**Status:** done
**Area:** Code structure

## The problem

`caelumora-runtime/src/external_impl/a2a_agent.rs` (925 lines, about 255 of them tests) holds four
things that hardly need each other:

| Part | Lines, about | What | Depends on |
|---|---|---|---|
| Wire types | 170 | `RpcResponse`, `RpcError`, `AgentCard`, `AgentInterface`, `AgentCapabilities`, `StreamResponse`, `WireTask`, `WireStatus`, `WireTaskState`, `WireMessage`, `WirePart`, `WireArtifact`, `StatusUpdate`, `ArtifactUpdate` | serde |
| Translation | 105 | `Translation`, `status_events`, `text_of`: what the remote agent sends, as `ExternalEvent`s | wire types, `ExternalEvent` |
| Transport | 150 | `discover`, `post`, `call`, `stream`, `json_of`, `require_success`, `token`, `connection`, `events_of` (server-sent events) | reqwest, secrets, wire types |
| Component | 150 | `A2aAgentConfig`, `A2aAgentComponent`, `impl ExternalAgent` (`send`, `resume`, `cancel`), `polled`, `register` | all of the above |

Besides the size, the mix shows in the code:

- **What comes in is typed, what goes out is not.** `send` builds its message and its parameters
  with `json!`, and the JSON-RPC methods (`"SendMessage"`, `"SendStreamingMessage"`, `"GetTask"`,
  `"SubscribeToTask"`, `"CancelTask"`) are string literals in four functions.
- **The transport translates.** `events_of` frames server-sent events, parses JSON-RPC *and* turns
  the result into `ExternalEvent`s; `answered` and `polled` translate on their own. The transport
  cannot be read or tested without the translation.
- **The translation has no tests of its own.** It is logic with state and no I/O - an artifact sent
  in chunks, a task or a conversation that changes - but is only tested through an HTTP stub.
- **The component is `Clone` for one reason:** `polled` moves a copy of it into its stream to call
  `GetTask`.
- **The agent's URL is handed around for messages only:** `into_result(endpoint)`,
  `events_of(bytes, endpoint)`, and `self.shown()` at a dozen places.
- **`cancel` rebuilds half of `call`,** since `RpcResponse::into_result` writes the error code into
  the message, after which `TASK_NOT_CANCELABLE` cannot be told any more.
- **Two endpoints:** `Endpoint { url, streaming }` is what was read from the card, `RemoteEndpoint`
  what was configured; the card calls the former an interface.

## Why it matters

- [204](204-external-agent-roster.md) (a roster for the remote agent, as `metadata` or a data part)
  and [208](208-generated-artifacts.md) (file and data parts instead of text only) both change
  what is sent and read. Today that means editing `json!` literals in `send` and a wire type 300
  lines further down.
- A second binding of A2A (gRPC, HTTP+JSON) or a second version would share the translation and
  replace the transport; today they are one.

## Decided

On 2026-10-03, with the maintainer: the file becomes a module with a file per part, the outgoing
side is typed, and the transport becomes a client of its own (`A2aClient`) that knows nothing of
`ExternalEvent`. Three steps, each a commit of its own.

## Plan

### The target

```
caelumora-runtime/src/external_impl/a2a_agent/
    mod.rs          the component: A2aAgentConfig, A2aAgentComponent, impl ExternalAgent,
                    polled, register; the tests against the HTTP stub
    wire.rs         the protocol: every type that is sent or read, the constants, the methods
    translation.rs  between the wire and `crate::external`: Translation (in), the message
                    that is sent (out)
    client.rs       A2aClient: the agent card, JSON-RPC over HTTP, server-sent events
```

The module keeps its name and path (`external_impl::a2a_agent`), so `external_impl/mod.rs`, the
component kind and COMPONENTS.md stay as they are; only the module comment of `external_impl`
("one file per component kind") becomes "one module per component kind". Everything inside is
`pub(super)` at most; `A2A_AGENT_KIND`, `A2aAgentComponent` and `register` stay `pub`.

Dependencies point one way: `wire` knows nothing of the others; `translation` knows `wire` and
`crate::external`; `client` knows `wire` and `crate::remote`; `mod` knows all three.

### Step 1 — `wire.rs` and `translation.rs`, moved

No change of behaviour, no change of names.

- `a2a_agent.rs` becomes `a2a_agent/mod.rs` (`git mv`, so that the history follows).
- To `wire.rs`: the fourteen wire types with their `impl`s (`RpcResponse::into_result`,
  `AgentCard::offered`, `WireMessage::text`, `WireTaskState::works`), and what belongs to the
  protocol rather than to this client: `A2A_VERSION`, `CARD_PATH`, `JSONRPC_BINDING`,
  `TASK_NOT_CANCELABLE`, `is_version_1`.
- To `translation.rs`: `Translation`, `status_events`, `text_of`. `WireMessage::text` calls
  `text_of`; since a message's text is a matter of the wire, `text_of` goes to `wire.rs` if that
  avoids `wire` depending on `translation` (it does: it stays with `WirePart`).
- What stays in `mod.rs` for now: the configuration, the component with its transport methods,
  `events_of`, `answered`, `polled`, `register`, `POLL_FIRST`, `POLL_LONGEST`, `DEFAULT_TIMEOUT`,
  and all tests.
- New tests in `translation.rs`, which the move makes possible - each on `Translation::events`
  with hand-made `StreamResponse`s (parsed from JSON, as the wire types only deserialize):
  - an artifact in chunks: `Partial` per piece, the whole as one `Message` at `lastChunk`; a chunk
    without `append` starts over;
  - the conversation and the task are told once, and again when they change;
  - a message without a task is the whole answer (`Message`, `Completed`);
  - every `WireTaskState`: what it ends with, and the fallback texts of `Failed`, `Rejected`,
    `InputRequired`; a state not known is `Other` and tells nothing;
  - `started` tells the conversation and the task and nothing of the task's artifacts.

Size: small. About 280 lines move; about 80 lines of tests are new.

### Step 2 — what is sent is typed

The JSON that is sent stays the same, byte for byte in its fields; the existing tests against the
stub assert the fields and keep passing unchanged.

- **One `WireMessage` for both directions** rather than a second type: it gains `message_id`,
  `role` and `task_id`, derives `Serialize` too, and leaves out what is `None`
  (`skip_serializing_if`). `WirePart` derives `Serialize`. A new `enum WireRole { User, Agent }`
  (`ROLE_USER`, `ROLE_AGENT`), since the role is sent now and will be read once the context of a
  message is shown ([205](205-external-agent-message-context.md)).
- **The parameters** as types in `wire.rs`: `SendMessageParams { message, configuration }`,
  `SendConfiguration { accepted_output_modes, return_immediately }`, `TaskParams { id }` (for
  `GetTask`, `SubscribeToTask`, `CancelTask`), and the envelope
  `RpcRequest<P> { jsonrpc, id, method, params }`.
- **The methods** as constants in `wire.rs` (`method::SEND_MESSAGE`, ...), used by the component
  and by the tests.
- **Building the message** moves from `send` to `translation.rs`, beside its counterpart: a
  function from `OutboundMessage` and the conversation to `WireMessage` - one text part per thing
  said, as `"{sender}: {text}"`. That line is what 204 and 208 will change.
- `post`, `call` and `stream` take `params: impl Serialize` instead of a `Value`.
- New tests: in `wire.rs`, that a message and each parameter type serialize to exactly the JSON
  written by hand today (compared with a `json!` literal), `None`s left out; in `translation.rs`,
  the message built for an `OutboundMessage` with and without conversation and answered task.

Size: small. About 60 lines of types, about 20 lines less in `send`.

### Step 3 — `A2aClient`

The transport becomes a type of its own in `client.rs`, held by the component as
`Arc<A2aClient>`. Its responsibility in one sentence: it reaches one A2A agent over JSON-RPC and
hands back what the agent answered, as wire types.

```rust
pub(super) struct A2aClient {
    id: String,                    // the component's, for the log
    endpoint: RemoteEndpoint,      // as configured
    timeout: Duration,
    http: reqwest::Client,
    secret_manager: Arc<SecretManager>,
    interface: ArcSwapOption<Interface>,   // read from the card, once
}

/// What of the agent card is used (today's `Endpoint`).
pub(super) struct Interface { pub url: String, pub streaming: bool }

impl A2aClient {
    pub(super) fn new(id: String, endpoint: RemoteEndpoint, timeout: Duration,
                      secret_manager: Arc<SecretManager>) -> Result<Self, ComponentError>;
    /// Whether the agent streams; reads the card the first time.
    pub(super) async fn streaming(&self, security: &SecurityContext) -> ExternalAgentResult<bool>;
    /// The result of `method`, or the JSON-RPC error it was answered with.
    pub(super) async fn call<T: DeserializeOwned>(&self, security: &SecurityContext, method: &str,
        params: impl Serialize) -> ExternalAgentResult<Result<T, RpcError>>;
    /// The responses of the streamed `method`, as they come.
    pub(super) async fn stream(&self, security: &SecurityContext, method: &str,
        params: impl Serialize) -> ExternalAgentResult<BoxStream<'static, ExternalAgentResult<StreamResponse>>>;
    /// The error for a request the agent refused with `error`.
    pub(super) fn rejected(&self, error: RpcError) -> ExternalAgentError;
}
```

- **Moves into the client:** `discover`, `endpoint` (as `interface`), `token`, `connection`,
  `json_of`, `require_success`, `post`, `call`, `stream`, `card_url`, and `events_of` as
  `responses_of` - framing and JSON-RPC only, yielding `StreamResponse`. The client resolves the
  interface itself, so no caller passes an `Endpoint` any more.
- **`call` returns the JSON-RPC error as a value** (`Result<T, RpcError>` inside the
  `ExternalAgentResult`), so that `cancel` uses `call` like everybody else and matches on
  `TASK_NOT_CANCELABLE`; the others turn it into `Rejected` with `client.rejected(error)`.
  `RpcResponse::into_result` returns that `Result<T, RpcError>` and needs the endpoint only for
  "neither a result nor an error". No new error type, and `ExternalAgentError` is not changed.
- **Translation in the component, in one function** for what the client hands back: `translated`
  takes a stream of `StreamResponse`s - the server-sent events, or the one answer of
  `SendMessage` - and yields `ExternalEvent`s through one `Translation`, ending after an error.
  `answered` and the translating half of `events_of` become this.
- **`polled` keeps a `Translation` of its own.** While a task works it tells only that it began
  (`Translation::started`), not what `events` would make of each `GetTask` answer - every answer
  repeats the task's artifacts so far. It takes the `Arc<A2aClient>` instead of a copy of the
  component, which then need not be `Clone`.
- **The component that is left:** `id`, `client`, `classification`. `build` makes the client and
  tries the card (a failure is logged, as today); `send`, `resume` and `cancel` each say which
  method with which parameters, and whether streamed, polled or answered at once.
- **Tests:** `events_split_anywhere_by_the_network_arrive_whole` goes to `client.rs` and asserts
  the `StreamResponse`s that arrive (which task, which artifact text, which state) instead of
  events. The tests against the stub stay in `mod.rs`, unchanged in what they assert; only how the
  component is built changes. `the_parameters_name_the_agent_and_how_to_reach_it` stays with the
  configuration, its `card_url` part with the client.

Size: medium. About 150 lines move and change their signatures; the component shrinks to about
120 lines. The one step with a risk: error paths (a refused stream answered with JSON instead of
events, a timeout before the stream begins, a broken stream) must end up as the same
`ExternalAgentError`s with the same texts - the stub tests cover the first two and the refusals;
a test for a stream that breaks off in the middle is added in `client.rs`.

### What to watch

- **No change of behaviour in any step.** What is sent, what is told as `ExternalEvent`s, the
  error messages and what is logged at which level stay as they are. Message content stays at
  `trace!` only.
- **Error messages name the agent without credentials** (`RemoteEndpoint::shown`): the client is
  now the one place that does, which the test with `user:secret@` keeps checking.
- **`#[serde(deny_unknown_fields)]`** stays on the configuration only; the wire types keep ignoring
  what they do not read, since a remote agent may send more.
- **Few types** (AGENTS.md): new are `A2aClient`, `WireRole`, three parameter types and the request
  envelope; `Endpoint` is renamed, not added. One `WireMessage` serves both directions. `call`
  returns a nested `Result` rather than a new error type. Each new type is named above with what
  it is for.
- **Not part of this issue:** file and data parts (208), the roster (204), a second binding or
  version. The split is what makes them local changes.

### What it costs

About 925 lines become four files of roughly 250 (`mod.rs`, with the stub tests), 230 (`wire.rs`),
220 (`translation.rs`) and 230 (`client.rs`) - more lines in sum, by the new tests of the
translation and the wire. No other file changes besides the module comment in
`external_impl/mod.rs` and a line in COMPONENTS.md. Steps 1 and 2 are small and mechanical; step 3
is the restructuring and is reviewed on its own.

## Done when

- `external_impl/a2a_agent/` holds the protocol's types, the translation, the client and the
  component in a file each, depending on each other one way.
- Nothing that is sent is built with `json!` outside the tests, and no method name is a string
  literal outside `wire.rs`.
- The client hands back wire types and knows nothing of `ExternalEvent`; the translation is tested
  without HTTP.
- `cancel` uses `call`, and the component is not `Clone`.
- COMPONENTS.md names the module's parts.

## History

- **2026-10-03** — Issue created with the plan, after reading the file with the maintainer; all
  three steps decided. One point of the first proposal is corrected in the plan: polling keeps a
  translation of its own, so two of the three sources of answers are translated in one place, not
  all three.
- **2026-10-03** — Step 1 done: `a2a_agent.rs` is `a2a_agent/mod.rs` (moved, its history kept),
  with `wire.rs` (the wire types, the protocol's constants, `is_version_1`, `text_of`) and
  `translation.rs` (`Translation`, `status_events`) beside it; `Translation` keeps its state and
  helpers private. Five tests of the translation without HTTP are new: an artifact in chunks, the
  conversation and the task told once and again when they change, a message without a task, every
  task state with its fallback texts, a started task told without its artifacts.
- **2026-10-03** — Step 2 done: what is sent is typed in `wire.rs` - `RpcRequest`,
  `SendMessageParams`, `SendConfiguration`, `TaskParams`, the methods in `wire::method` - and one
  `WireMessage` serves both directions, with `message_id`, `role` and `task_id` added and left out
  when not set. `post`, `call` and `stream` take `impl Serialize`; `cancel` reads its result as
  `IgnoredAny`. `translation::wire_message` builds the message from an `OutboundMessage`, beside
  `ACCEPTED_OUTPUT_MODES` (only text is read). Two deviations from the plan: `WireRole` has a third
  variant, `Unspecified` (`ROLE_UNSPECIFIED`, and any role not known), so that a message with a
  role of a later version is still read; and the tests against the stub keep the method names as
  literals, on purpose - they assert the protocol, which a constant written wrong would not catch.
  Four tests are new: the request envelope, a message and its parameters as written by hand before,
  a role not known, and the message built for what is said.
- **2026-10-03** — Step 3 done, and with it the issue. `client.rs` holds `A2aClient`: the card
  (read once, `Interface`, formerly `Endpoint`), JSON-RPC over HTTP and server-sent events
  (`responses_of`, formerly the framing half of `events_of`), handing back wire types. `call`
  returns the JSON-RPC error as a value (`Result<T, RpcError>`), `stream` the responses
  (`Responses`); `RpcError::rejected` makes a refusal an `ExternalAgentError`. The component is
  `client` and `classification`, not `Clone`; `translated` turns the responses of a stream or of
  one answer into events through one `Translation`, `polled` keeps its own and holds the client.
  `cancel` uses `call`. One change of what users read: a refused cancel names the JSON-RPC code
  too, like every other refused request (`boom (-32603)`), where it gave the message alone. The
  stream test moved to the client and asserts responses; new are tests of where the card is
  found and of a stream that breaks off or is answered with an error, which ends after it. Files:
  `mod.rs` 453 lines (about 250 of them tests), `wire.rs` 326, `translation.rs` 229, `client.rs`
  312.
