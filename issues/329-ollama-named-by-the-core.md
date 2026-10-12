# 329 — The core names Ollama as the default model

**Status:** open
**Area:** Code structure

## The problem

The one language model Caelumora can reach is the component kind `ollama_client`
(`caelumora-runtime/src/llm/ollama_client.rs`, about 1150 lines, with `control_tokens.rs`). It is
registered like any other kind, but `llm::register_all` also makes it the built-in default of the
`LLMClient` facet:

```rust
registry.single::<dyn LLMClient>(Some(ollama_client::OLLAMA_CLIENT_KIND));
```

A personality that names no model gets Ollama unless `component_defaults` names another. So the
core does not only contain one backend of the facet, it names it, and a second model kind - an
OpenAI-compatible or the Anthropic API - would be a second kind beside it in the core, or a crate
that cannot become the default without the deployment configuring it.

## Why it matters

- The facet `LLMClient` is the seam between Caelumora and a model provider, as `ExternalAgent` is
  for remote agents (issue [326](326-a2a-in-a-crate-of-its-own.md)). There the core has no default
  and names no protocol; here it names one, which is a difference without a reason of its own -
  Ollama was the first and only provider.
- Which provider a deployment uses is a decision of the deployment; the core cannot know it.
- Not urgent: `ollama_client` brings no dependency that the core does not have anyway. It becomes
  pressing with the second model kind.

## Considerations

- **Who names the default.** Options:
  - *the core keeps a default, named by the kind's crate* when it registers (`single` with a
    default given by the plugin): the default is whichever crate registers last - an order that
    nobody sees. Rejected.
  - *the composition root names it* - `caelumora-app` and `caelumora-tui` register the model crates
    and say which one is the default, through the `RuntimeBuilder` of issue
    [323](323-kinds-enumerated-in-the-core.md); `component_defaults` in the configuration still
    overrides it. Recommended: the place where the plugins are chosen is the place where it is
    known which model kinds there are.
  - *no default at all*: every deployment configures `component_defaults`. Simple, but a
    configuration that worked without one breaks.
- **Control tokens.** `control_tokens.rs` defuses the control tokens of a model whose backend
  tokenizes with its special tokens enabled (issue 149). That is a property of a backend, not of
  Ollama alone; it stays in the core as something any model kind may use, or moves with Ollama if
  no other kind needs it. Decided with the second model kind.
- **The core's tests.** Containers built in the core's tests that name no model get Ollama today
  and never call it. Without a default in the core they fail to build; they name a scripted model
  instead, which says what they rely on.
- **The crate** is `caelumora-ollama`, of the shape of issue
  [325](325-mcp-in-a-crate-of-its-own.md).

## Done when

- The core names no model kind; `caelumora-ollama` holds `ollama_client`, and the binaries name it
  as the default where they list it.
- A configuration without `component_defaults` gives the same model as before.
- `docs/COMPONENTS.md`, `docs/CONFIGURATION.md` and `docs/LLM-MODELS.md` say where the default
  comes from.

## History

- **2026-10-12** — Opened from the discussion of which subjects besides skills could move into
  crates of their own (issue 323).
