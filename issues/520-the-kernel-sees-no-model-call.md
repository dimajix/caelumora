# 520 — The kernel sees no call of a model

**Status:** open
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
