# 520 — The kernel sees no call of a model

**Status:** in progress
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** A language model is called by the library that brings it, from inside the agent, so no call of a model passes the kernel: it does not know who calls which model in whose name, and cannot count, limit or refuse a call.

## The problem

An agent asks its model through the facet `LLMClient` (`caelumora-agents/src/llm/mod.rs`). Its
only kind in production is `ollama_client`, a component of the library `caelumora-ollama`
(`OllamaLibrary`, issue [516](516-the-runtime-trusts-the-agents-linked-into-it.md)): the
component holds the endpoint of the Ollama server and its own HTTP client, and the agent's turn -
and the compaction by summary (`caelumora-compaction/src/summary_compaction.rs`), which `needs`
the facet too - call it directly. The kernel builds the component and is out of the picture from
then on.

Under the kernel view of issue 516 a model is a resource like the workspace or the secrets: costly,
shared by every agent of the system, and reached with credentials that are the operator's. Every
other such resource is a service of the kernel - with backends brought by plugins
(`Backends::register`, the `TerminalProvider` of the workspace, the `SecretProvider`s) - and is
reached by an agent through a call the kernel answers in the context it knows. The model is the
exception: it is a library's, trusted for nothing, and the kernel never sees a call of it.

## Why it matters

The goal behind it is an **accounting of what the models are used for** - tokens and calls per
user, agent, session and model, and later budgets that refuse a call. That needs one place every
call passes, which knows in whose name the call is made and what it used. Today there is none: the
tokens are recorded on the messages of each agent's transcript (issue [209](209-token-usage.md)),
a call that produces no message is counted with another one or not at all (issue
[514](514-calls-without-a-message-of-their-own.md)), and a library could call a model without
anything being recorded.

**The accounting itself is not part of this issue.** This issue makes the model a service of the
kernel, so that the accounting can be built on it as an issue of its own; behaviour does not change.

What becomes possible besides, and is not part of this issue either: a model's endpoint and
credentials stay on the kernel's side and are never handed to a library; a model may be given a
clearance like a channel - a model served from a cloud admitting less than a local one - and the
kernel could check the context against it before it leaves.

## Considerations

### The pattern

The closest pattern is the workspace: a service of the kernel whose backends are brought by plugins
(`TerminalProvider`, registered with `Backends::register` and chosen by the configuration), and
which an agent's work reaches through what the kernel hands it in the work's context, never through
the backend itself ([PLUGINS.md](../docs/PLUGINS.md#what-a-system-service-offers-an-agents-work),
[WORKSPACE.md](../docs/WORKSPACE.md)). Accordingly:

- **A backend of models** - a trait of the kernel, implemented by `caelumora-ollama`, which becomes
  a plugin (a module of the kernel, with the kernel's trust) instead of a library. The backend
  holds the endpoint, the HTTP client and what is particular to its server: the defusing of control
  tokens (`control_tokens`, issue [149](149-special-tokens-in-content.md)) moves with it out of the
  agents' crate, where a library could skip it, into the kernel, where it cannot be skipped.
- **A call of the kernel** that an agent's work makes: context and tools in, a stream of
  `MessageEvent`s out, as `LLMClient::chat_completion` today. A stream crosses a process, as the
  terminal's `ByteStream` does, so the constraint of 516 holds. `LLMError` moves to the errors a
  kernel call answers (`caelumora_api::error`).

### Where a model is configured

- **A. As today, per agent.** The component's parameters - `endpoint`, `model`, `think`,
  `context_size`, ... - stay in the personality's configuration and are handed through to the
  kernel's service with every call, or when the agent is built. Least change; but an agent names
  endpoints, and there is no name of a model the accounting could count against or the operator
  could limit.
- **B. Named models of the kernel.** The configuration of the kernel declares the models there are,
  by name, each with its backend and its parameters (as storages and secret providers are declared);
  an agent names the model it uses. Endpoints and credentials are the operator's alone, and the
  name is what the accounting counts by.

Recommended: **B**, because the reason for the issue - one place that knows what is called - is
only half reached if an agent still says where its model is. It changes the format of the
configuration: the parameters of `ollama_client` in personalities and in
`AppConfig::component_defaults` move into the kernel's section; no compatibility for the old form.

### What stays the agent's

The `ollama_client` mixes two things: what its server needs (endpoint, model, tokenizer, `think`,
`context_size`, `vision`) and how the agent frames its context for that model (`sender_format`,
`sender_style`, `message_times`, `reasoning_history`, `system_messages`, `prompt_texts` - the
`ModelProfile` it provides beside the `LLMClient`). The first goes to the kernel's model; the
second is the agent's way of writing its context and stays on the agent's side. Where each of the
mixed ones lands - `vision` and `context_size` are facts about the model that the profile also
reads - is decided in the plan. Issue [429](429-message-framing-configured-three-times.md), the
framing declared three times, touches the same parameters and may be resolved with it.

### What becomes of the facet `LLMClient`

- **A. The facet stays** as the agent's interface to its model, and its kind in production becomes
  a component of the agents' library that only forwards to the kernel's call. The compaction and
  the tests with scripted models (`TestModel`, the compaction's `Summariser`) stay as they are; but
  a library could still bring its own `LLMClient` past the kernel.
- **B. The facet goes.** The agent and the compaction call the kernel directly; a scripted model of
  a test becomes a backend registered with the test's kernel. One way less, and no model call that
  does not pass the kernel - as far as linked code can be kept from making HTTP requests at all,
  which, as 516 weighed for tools with an effect outside the system, it cannot.

To be decided in the plan; B is the simpler result if the tests' scripted models fit as backends.

### In whose name

The call is made in the work's context, which the kernel knows - through the `Turn` the work acts
by, as the workspace's writer is handed out there - so that a later accounting can attribute it to
the actor, its owner, the session and the channel without the agent saying so. A call the work
makes outside a turn, if there is one, is to be found in the plan.

### Out of scope

- The accounting, its storage, its views and budgets: an issue of its own, built on this one.
- The decision models of `caelumora-decision` (issue [403](403-decision-model-contract.md)): a
  different contract (`POST /v1/systemone`), called by a module of the kernel already.
- The external agent (`caelumora-a2a`): it is an agent elsewhere, not a model.

## The plan

Agreed with the maintainer on 2026-11-02. It revises the considerations above: the facet stays,
and what the kernel hands an agent is a client of its own, built for it alone.

### The pattern

The secrets: a manager of the kernel over backends that plugins bring (`SecretManager` over
`SecretProvider`), which hands out a handle that acts in a context and asks the policy at every
use (`SecretManager::for_context`, `ActorRuntime::secrets`). A model is the same: a backend that
knows its server, a manager that decides, and a client that is a handle of the kernel.

### Three types

- **`ModelProvider`** (`caelumora-kernel`, `model`) - the backend: one configured model, served by
  the backend its `type` names. It knows its server - the endpoint, the HTTP client, the control
  tokens (issue 149), what is cached per server - and decides nothing.

  ```rust
  /// A language model the kernel offers: one entry of the configuration's `models` (issue 520).
  #[async_trait]
  pub trait ModelProvider: Send + Sync + Debug {
      /// Whether the model is given images - configured, or asked of its server once.
      async fn sees_images(&self) -> bool;
      /// Sends `context` and `tools` and streams the answer: partial messages, then the complete
      /// ones, built on `unlabelled_message` and carrying the measures of the call. Defuses the
      /// model's control tokens before anything is sent.
      async fn chat_completion(
          &self,
          context: &[Message],
          tools: &[Tool],
      ) -> BoxStream<'static, ModelResult<MessageEvent>>;
  }
  ```

  Registered by plugins with `Backends::register::<dyn ModelProvider, _, _>` and configured by name
  like the storages (`NamedBackends`):

  ```yaml
  models:
    gemma:       { type: ollama, endpoint: { url: http://localhost:11434 }, model: gemma4:27b }
    gemma-cloud: { type: ollama, model: gemma4:cloud, tokenizer: gemma4:27b }
  default_model: gemma
  ```

- **`ModelManager`** (`caelumora-kernel`, `model`) - the service of the kernel: the models by
  their names, and the clients of them. Where a later issue checks which agent may use which model
  and counts what it uses; in this issue it resolves the name and builds the client.

  ```rust
  impl ModelManager {
      /// The models `models` configures, `default` the one an agent uses that names none.
      pub fn new(models: NamedBackends<dyn ModelProvider>, default: Option<String>) -> Self;
      /// The client of `model` - the default where `None` - for the one actor `acting` acts for,
      /// each call made in the context `acting` builds at that call (issue 520).
      ///
      /// # Errors
      /// `ModelError::Unknown` for a model not configured.
      pub(crate) fn client_for(
          self: &Arc<Self>,
          acting: Acting,
          model: Option<&str>,
      ) -> ModelResult<AgentModelClient>;
  }
  ```

- **`ModelClient`** (`caelumora-api`, `model`; today `LLMClient`) - the facet through which an
  agent asks its model, implemented by the kernel's client (`AgentModelClient`) alone.

  ```rust
  #[async_trait]
  pub trait ModelClient: Send + Sync + Debug {
      /// Sends `context` and the `tools` the model may call, in the agent's name for `initiator`,
      /// and streams its answer as `MessageEvent`s. The stream owns the request; dropping it
      /// abandons it.
      async fn chat_completion(
          &self,
          initiator: ActorId,
          context: &[Message],
          tools: &[Tool],
      ) -> BoxStream<'static, ModelResult<MessageEvent>>;
  }
  ```

### In whose name a call is made

Not decided here, and kept open: the client is built for one agent, and forms the context of each
call when it is made, as `ParticipantRuntime::acting_for(initiator)` does - the agent's rights as
they are then, and the initiator of the work. So the context of a call names the initiator, the
caller, the owner, the principal, the session and the channel, and whatever a later check or
accounting counts by is there. The client keeps no `SecurityContext` from when it was built: the
context is built from the participant at each call (`ResolvedParticipant::context_in`), and one
built once would grow stale. The initiator is passed with every call, as every call of the kernel
in the work's name is told it (`Situation::initiator`).

### `Acting`: in whose name an actor acts

What builds that context exists, but not as a type of its own: `ParticipantRuntime` holds the
actor, its session and channel and an enum `Acting` - the resolved participant with the
controllers of its user or its agent and personality, or the approver; in tests `Itself` or
`Fixed` -, and `acting_for` builds the context from them and the channel as it is now. `Acting`
becomes the type of that concept, with the method that builds the context:

```rust
/// In whose name an actor acts (issue 516): builds the context of each of its calls from what the
/// kernel holds of it now - never handed in by the actor.
#[derive(Debug, Clone)]
pub(crate) struct Acting { … } // the channel's controller and the resolved participant; the test variants inside

impl Acting {
    /// The context the actor acts in now, for `initiator` - who began the chain it answers, for the
    /// record, never for rights.
    pub(crate) fn context_for(&self, initiator: ActorId) -> SecurityContext;
}
```

It holds handles of the controllers, not ids, so that every context it builds is as the
controllers are then: a role taken away or a personality that limits more applies at the next
call. Its users:

- **`ParticipantRuntime`** holds an `Acting`; `acting_for` becomes `self.acting.context_for(…)`.
- **`BuildContext`** holds an `Acting` instead of a context built once; planning the components
  (`ComponentContainer`, today the only reader of that context) asks `context_for(actor)`. The
  kinds of the kernel reach it through the `BuildContext`; the API's `ComponentContext` does not
  carry it.
- **`ModelManager::client_for`** takes it; the client keeps it and builds the context of each call.

Revised on 2026-11-02, with the maintainer, when step 2 was begun: a `BuildContext` is made not
only for an agent's components but for a channel's (in its owner's name), a session's commands
and in the system's name, where there is no participant, only a context given once. So `Acting`
stays an enum and gains a variant for that, `Given(SecurityContext)` - the test-only `Fixed` of
before, now one of production -, beside `Participant` and the test's `Itself`; it answers
`context()`, in its own name, beside `context_for(initiator)`. `BuildContext::new(context,
services)` stays for every caller of before and makes a `Given`; `BuildContext::acting_as(acting,
services)` is the agent's. Weighed and not chosen: an `Option<Acting>` beside the context in
`BuildContext`, set for agents alone, and leaving `BuildContext` as it is and handing the `Acting`
to the kind `model` as a service only an agent's build offers.

`Acting` is `pub(crate)` in the kernel: whoever holds it can make contexts in the participant's
name. It is never part of `caelumora-api` nor handed to a library, and no plugin needs it - a
backend of models decides nothing.

### How the agent gets its client

As a component, as today: the kind `model`, registered by the kernel - no longer by a library -,
in the agent's components instead of `ollama_client`. Its parameters: `name`, a model of
`models` (the default left out), and the framing (`sender_format`, `sender_style`,
`message_times`, `reasoning_history`, `system_messages`, `prompt_texts`). It provides
`ModelClient`, built by `ModelManager::client_for` while the agent's components are built
(`ActorRuntime::build_components`), and `ModelProfileProvider`, its `images` asked of the model
(`sees_images`). The agent's actor and the compaction keep asking for the facet as they do; they
pass the initiator of their turn.

What this needs of the kernel:

- **A kind of the kernel builds for one agent.** Its factory takes the agent's `Acting` from the
  `BuildContext` and hands it to `ModelManager::client_for`. It is the first kind of the kernel
  that provides a facet of an agent; the mechanism allows it, PLUGINS.md is to say so.
- **No library brings a `ModelClient`.** `libraries_keep_to_their_side` refuses, as it refuses a
  kind of a facet the kernel builds in its own name, a library's kind that provides `ModelClient`,
  and a library that declares it a single - so that no call of a model passes the kernel.
- **`ModelProfileProvider` moves to the API**, since a kind of the kernel provides it;
  `caelumora-a2a` keeps providing it for the external agent.

### Renames

The concept is called *model* everywhere else (`ModelProfile`, a model call, LLM-MODELS.md), so
in the same change: `LLMClient` → `ModelClient`, `LLMError` → `ModelError`, `LLMResult` →
`ModelResult`, `caelumora_agents::llm` → `caelumora_api::model` (with `unlabelled_message`).

### What moves

- `caelumora-ollama` becomes a plugin (`OllamaPlugin`) that registers the backend `ollama`; it no
  longer depends on `caelumora-agents`. `control_tokens` moves into it, its only user.
  `caelumora-app` and `caelumora-tui` list the plugin and no longer name a default kind of the
  facet: the kernel's kind `model` is the one there is.
- The scripted models of the tests (`TestModel` of the channel tests, the compaction's
  `Summariser`, the security suite) become a `ModelProvider` of the kernel's `test-support`.
- **Stored data:** personalities and `component_defaults` that configure `ollama_client` are to be
  rewritten to `model`, their endpoint and model moved to `models`; no compatibility for the old
  form.

### Steps

1. **Rename and move** (no behaviour change): `LLMClient`, `LLMError`, `LLMResult`,
   `unlabelled_message` and `ModelProfileProvider` to `caelumora_api::model` under their new names;
   the call takes the initiator.
2. **`Acting` as a type** (no behaviour change): drawn out of `ParticipantRuntime`, held by it and
   by `BuildContext` instead of the context built once.
3. **The service**: `ModelProvider`, `ModelManager`, the kind `model`, the guard against a
   library's `ModelClient`, `OllamaPlugin`, the scripted backend of the tests, the configuration;
   the documents (ARCHITECTURE.md, PLUGINS.md, AGENT-TURN.md, CONFIGURATION.md, CRATES.md,
   LLM-MODELS.md).

## Done when

- No library calls a language model: `caelumora-ollama` is a plugin that brings a backend of the
  kernel's model service, and every call of a model by an agent's work - the turn's and the
  compaction's - is a call of the kernel, made in the context the kernel knows.
- The defusing of control tokens is done by the backend, on the kernel's side.
- Behaviour is unchanged; the documents (ARCHITECTURE.md, PLUGINS.md, AGENT-TURN.md,
  CONFIGURATION.md, CRATES.md) describe the model as a service of the kernel.

## History

- **2026-11-01** — Opened at the maintainer's request: the model is to be a service of the kernel
  rather than a library, so that an accounting of its use can be built in the kernel later.
- **2026-11-02** — Planned with the maintainer: a `ModelManager` of the kernel over the backends
  (`ModelProvider`) builds each agent a client of its own (`ModelClient`, today `LLMClient`), which
  forms the context of every call when it is made; in whose name a call is counted is left open.
  What builds that context becomes a type of its own, `Acting`, drawn out of `ParticipantRuntime`
  as a step before the service.
- **2026-11-02** — Step 1 done on the branch: `ModelClient`, `ModelError`, `ModelResult`,
  `unlabelled_message` and `ModelProfileProvider` are in `caelumora_api::model`, the call takes the
  initiator, a `ModelError` is judged for users by the API; `control_tokens` moved into
  `caelumora-ollama`, its only user, which no longer depends on `caelumora-agents`. The texts of
  `ModelError` users see still say "LLM backend"; they change with step 3, where the errors of a
  backend are rewritten anyway.
- **2026-11-02** — Step 2 done on the branch: `Acting` is a type of its own (`kernel/acting.rs`),
  with `Given` beside `Participant` and the test's `Itself`, as revised in *The plan*;
  `ParticipantRuntime` builds its contexts by it, and `BuildContext` holds it - an agent's
  components are built in the agent's `Acting`, all others in a context given once, as before.
- **2026-11-02** — Step 3 done on the branch, as decided with the maintainer before it began:
  `AppConfig::models` names the models, each a backend (`ModelProvider`) by its name, flat and
  without a key for a default - an agent's model names one, or talks to the one there is; the
  `ModelManager` of the kernel builds each agent its client (`ModelClientFor`), which builds the
  context of each call from the agent's `Acting` and logs who asks which model; the kind `model`
  of the kernel, the default of both the model's facets, takes the framing as its parameters and
  reaches the `Acting` as a service its `BuildContext` offers; no library may bring a
  `ModelClient`. `caelumora-ollama` is a plugin with the backend `ollama`, and offers the programs
  a local model where none is configured (`with_local_model`); `ComponentRegistry::set_default_kind`
  had no user left and is gone. Deviating from the plan: the scripted models of the agents' tests
  stay `ModelClient` kinds the tests register in the kernel's registry - they test the agents, not
  the service, which has tests of its own -, and `TestAgents` is gone: a test's kernel offers a
  model that is not to be asked where the test configures none. Smaller deviations: the client is
  `ModelClientFor`, after `SecretsFor`, not `AgentModelClient`; `ModelManager::new` takes no
  default, since `models` has no key for one; and `client_for` answers a `ComponentError` naming
  the parameter `name`, as the attachments' storage is refused, not a `ModelError::Unknown` - an
  unknown model is a configuration that cannot be built, not a call that failed. The texts of
  `ModelError` say "model server" now.
