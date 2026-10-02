# 149 — Special tokens typed into a message reach the model as control tokens

**Status:** done
**Area:** Identity and access management

## The problem

Ollama renders a chat into one prompt and tokenizes it with the model's special tokens enabled -
also inside the content of a message. What a participant types as `<|im_start|>` (qwen3),
`<start_of_turn>` (gemma3), `<|start_header_id|>` (llama3.1) or `<|start|>` (gpt-oss) becomes the
real control token, not text. Measured on 2026-09-30 through the prompt token count: each costs one
token, the same text with `(`/`)` instead of `<`/`>` costs five to seven
(`work/sender_names/special_tokens.py`).

`OllamaChatMessage::from` (`caelumora-runtime/src/component_impl/ollama_client.rs`) passes the
content through as it is. So a participant can end their own message and open a real new turn from
inside it - a system message, or an answer "of the agent" - that the model cannot tell from one the
runtime wrote. The same holds for everything else that ends up in the content: tool results (a
fetched web page, an MCP server's answer), notes, memory, knowledge.

## Why it matters

- It is the strongest form of prompt injection: not text that tries to persuade the model, but a
  turn boundary the model was trained to trust. A fake system message outranks the agent's actual
  instructions.
- It crosses the line the runtime draws between roles: the moderator rewrites other participants'
  messages to `user` (issue [135](135-sender-names-in-model-input.md)), and the context panel shows
  them as such, while the model sees something else.
- Tool results make it reachable without being a participant: anyone who can put text where a tool
  reads it.

## Considerations

- **Where it belongs.** In the Ollama client, not in the `ContextResolver`: which strings are
  control tokens is a property of the backend's tokenizer, and the OpenAI API tokenizes content
  without special tokens. The resolved context - what the context panel shows - stays as it is.
- **Which tokens.** `/api/show` with `verbose: true` lists the model's tokens with their types
  (`tokenizer.ggml.tokens`, `tokenizer.ggml.token_type`; type 3 is a control token). Fetched once
  per model, not per call. A fixed list per model family would go stale.
- **How to defuse.** Break each occurrence so it no longer matches the token (e.g. a zero-width
  space after the first character). The model then sees nearly the same characters - which matters
  for code and for talking about these formats - but as text. Removing them would change what the
  message says.
- **Also the other direction.** An answer of the model containing such a string (quoted from a
  message) goes into the transcript and is sent back the next time; the same cleaning applies to
  every role, not only `user`.
- gemma4 did not turn `<start_of_turn>` into a token; its own control tokens were not tested. The
  list from `/api/show` covers it either way.

## Done when

- No content of any message given to an Ollama model can produce one of the model's control tokens.
- A test shows a message with a control token reaching the backend as text.

## History

- **2026-09-30** — Issue created, found while testing how models handle sender names for
  [135](135-sender-names-in-model-input.md) (`work/sender_names/RESULTS.md`, finding 11).
- **2026-10-01** — Done. The `ollama_client` reads the model's tokens from `/api/show` (`verbose`)
  once per server and model, in a cache shared by the clients of a registry, and breaks every
  occurrence in the request with a zero-width space after its first character
  (`llm_impl/control_tokens.rs`, searched again until none is left). Defused is every text the
  template renders, of every role: content, reasoning, tool calls and arguments, tool results, and
  the names, descriptions and parameters of the tools offered. If the tokens cannot be read, the
  call fails (fail closed); a failed read is not kept. The resolved context stays as written.
  Measured beforehand on qwen3, gemma4 and gpt-oss: type 3 is not enough - user-defined tokens
  (type 4) are tokenized alike, e.g. qwen3's `<think>`, `</think>`, `<tool_call>`, `<tool_response>`
  and gpt-oss's `[PAD0]`; both types are defused. Across eight models every such token is bracketed,
  so no ordinary text is touched. With the zero-width space, `<|im_start|>` costs qwen3:8b seven
  prompt tokens instead of one, checked through the client against a real Ollama. A model that does
  not list its tokens cannot be used with the `ollama_client` any more. Tests:
  `control_tokens::tests`, and `a_control_token_in_a_message_reaches_ollama_as_text` /
  `without_the_model_s_tokens_nothing_is_sent` against a stand-in server.
