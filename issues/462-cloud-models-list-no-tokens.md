# 462 — A cloud model served through Ollama lists no tokens, so the `ollama_client` refuses every call

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** Integrations
**Summary:** A model that Ollama serves from its cloud (`gemma4:cloud`) answers `/api/show` without its tokenizer, so the `ollama_client` cannot read its control tokens and, failing closed as issue 149 decided, refuses every call to it.

## The problem

Before the first call to a model, the `ollama_client` reads the model's tokens and their types from
`/api/show` (`verbose: true`), and breaks every control and user-defined token in what it sends
(issue [149](149-special-tokens-in-content.md), `read_control_tokens` in
`caelumora-ollama/src/client.rs`). A model that does not list them is refused: "the model … does
not list its tokens and their types, so its control tokens cannot be kept out of messages".

A cloud model is pulled as a stub that the local server forwards to `https://ollama.com`. Its
`/api/show` - measured on 2026-10-23 for `gemma4:cloud` - holds `capabilities`, `details`
(`family: gemma4`, `parent_model: gemma4`, BF16, 32.7B parameters) and four entries of
`model_info` (`general.architecture`, `general.parameter_count`, `gemma4.context_length`,
`gemma4.embedding_length`), but no `tokenizer.ggml.tokens`, no `tokenizer.ggml.token_type` and no
template. The local `gemma4:31b` and `gemma4:26b` list 262144 tokens each, 24 of them of the types
3 and 4.

So no cloud model can be used with the `ollama_client`: every turn of an agent on one fails.

## Why it matters

- Cloud models are how a deployment uses a model too large for its own hardware, through the same
  server and the same API.
- Failing closed was chosen on purpose: otherwise the content of a message can open a fake turn.
  Whether the cloud tokenizes special tokens in content as a local server does is not known; it
  renders the same chat templates, so it has to be assumed.

## Considerations

- **Measure first.** Whether the cloud turns a control token in content into the token can be
  measured as for issue 149: the prompt token count of a message holding one of gemma4's control
  tokens, against the same text with `(` and `)` for `<` and `>`. If it does not, the cloud model
  needs no defusing - but that is a property of a service, which may change unannounced.
- **The tokens of another model.** A parameter naming the model whose tokens are read in place of
  the configured one's - `gemma4:31b` for `gemma4:cloud` -: the operator knows which tokenizer the
  cloud model shares. It needs that model on the server, and is as current as it is; if that one
  does not list them either, the call still fails.
- **The tokens in the configuration.** A parameter listing the control tokens themselves. It needs
  no second model, but goes stale - which issue 149 rejected for a fixed list per family, though
  here the operator keeps the list, not the code.
- **Found by the family.** The client looks for a local model of the same `details.family` and
  reads its tokens. Nothing to configure, but a family is not a tokenizer, and which model is found
  depends on what happens to be pulled.
- **Defusing switched off.** A parameter by which the operator takes the risk for one model. The
  simplest, but for a model whose server does tokenize special tokens in content, it opens again
  what issue 149 closed.

## Done when

- A cloud model served through Ollama can be used with the `ollama_client`, with its control tokens
  kept out of what it is sent as far as they can be known, or knowingly not.
- `docs/CONFIGURATION.md` and `docs/LLM-MODELS.md` say how such a model is configured.

## History

- **2026-10-23** — Opened: `gemma4:cloud`, configured as the default model, refuses every call,
  since its `/api/show` lists no tokens.
- **2026-10-23** — Measured as the first consideration says, through the local server, with
  `num_predict: 1`: `<turn|><|turn>` in a message added 3 prompt tokens, the same text with `(` and
  `)` 7, the text defused by the client 9, and `<|tool_call>…<tool_call|>` 7 fewer than its
  bracketed form - for `gemma4:cloud` and the local `gemma4:e2b` alike. The cloud tokenizes
  control tokens in content as a local server does, so switching defusing off is ruled out, and
  the local gemma4 models share its tokenizer: `gemma4:e2b`, `gemma4:26b` and `gemma4:31b` list
  the very same 262144 tokens, 24 of them control tokens (Ollama 0.40.6).
- **2026-10-23** — Done as "the tokens of another model": the `ollama_client` has a parameter
  `tokenizer`, the model whose tokens are read for `model`'s control tokens, `model` itself unless
  configured. The cache of the control tokens is keyed by it; a model that lists no tokens is still
  refused, and the error names `tokenizer` as the way out - or, for a configured `tokenizer` that
  cannot be read, names it as the `tokenizer` of the model. Whether the two models share a
  tokenizer is not checked: Ollama tells nothing that would, and it is the operator's to know.
  `docs/CONFIGURATION.md`, `docs/COMPONENTS.md`, `docs/LLM-MODELS.md` (models of Ollama's cloud,
  with the measurement) and `examples/config.yaml` describe it. Test:
  `a_model_that_lists_no_tokens_is_defused_by_the_tokens_of_its_tokenizer`.
