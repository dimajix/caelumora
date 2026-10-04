# 209 — Token usage is not summed up, nor set against the model's context size

**Status:** in progress
**Area:** Observability and operations

## The problem

Ollama reports, with the last chunk of every answer, how many tokens the prompt had
(`prompt_eval_count`) and the answer (`eval_count`), and how long each took. `ollama_client` writes
them into the message it produces (`prompt_tokens`, `completion_tokens`, `prompt_duration`,
`completion_duration`, `total_duration`, `caelumora-model/src/message.rs`), and the web UI shows
them per message (`messages.ts`). That is all:

- **Nothing is summed up.** The numbers belong to one model call. A turn with tool calls or a plan
  is several calls, each with its own numbers, and its intermediate messages are in the transcript,
  not in the history the channel shows. There is no total per turn, per agent in a channel, or per
  channel.
- **The context size is unknown.** Neither the `LLMClient` nor its `ModelProfile` knows how large a
  context the model takes. `ollama_client` sets no `num_ctx`, so Ollama's default for the model
  applies - which may be far below what the model was trained for - and Ollama cuts a longer
  prompt off without telling. How full an agent's context is cannot be seen.

## What is wanted

The token usage summed up per agent and per channel and shown in the web UI - best set against the
maximal context size of the agent's model, so that one sees how close an agent comes to its limit.

## Considerations

### Two different numbers

- **Consumption:** the sum of prompt and completion tokens over all of an agent's model calls in a
  channel - what it cost. Grows with every call; says nothing about the limit, since every call
  sends the whole context again.
- **Fill level:** the prompt tokens of the agent's last call (plus its answer), against the
  context size - how much of its context is in use. This is the number that belongs in relation to
  the context size; a sum against it means nothing.

Both are wanted; the issue has to keep them apart in the UI.

### Where the sums come from

The numbers are recorded already, in every message an agent's model produced, and every such
message is kept in the agent's transcript - intermediate steps included. Following "reuse what
exists" (as decided for [205](205-external-agent-message-context.md)), the sums are computed from
the transcript when they are read, not kept as a counter of their own. If that turns out too slow
for long channels, a running total kept in the agent's channel state, beside the transcript, is the
next step. Turns given up (issue 145) stay in the transcript as branches and count as well - their
calls were made.

Per channel is the sum over its agents. External agents report no tokens (A2A has no such field)
and are shown as unknown, not as zero.

### Where the context size comes from

- **Configured:** a parameter of `ollama_client`, e.g. `context_size`, which is sent to Ollama as
  `options.num_ctx` - so that the size shown is the size used, and a model is no longer silently
  limited to Ollama's default.
- **Read:** `/api/show`, which `ollama_client` reads already for the control tokens (issue 149),
  holds the model's trained maximum (`<architecture>.context_length` in `model_info`) and, if the
  model file sets one, its `num_ctx`. The client keeps it with the control tokens.
- The `LLMClient` hands it out - in its `ModelProfile` or by a method of its own - so that the
  runtime does not know Ollama.

Probably both: read the maximum, use the configured size if one is given (refusing one above the
maximum), otherwise Ollama's default, which then has to be known too.

### Cached prompts

Ollama keeps the prompt of the previous call and evaluates only what is new. Whether
`prompt_eval_count` then counts the whole prompt or only the newly evaluated tokens has to be
measured before the fill level relies on it; if the latter, the fill level needs another source
(e.g. counting with `/api/tokenize`, or the sum of what was evaluated since the context was built
anew).

### REST API and web UI

- The member snapshot (or the agent's context view, under `ReadAgentContext`) carries per agent:
  consumption, fill level, context size. The channel carries the sum of its agents' consumption.
- The member list shows a bar per agent: last prompt against context size, with a warning once it
  comes close; the totals in a tooltip or in the channel's header.
- Whether consumption is visible to every participant or only to who may read the agent's context
  is to be decided; token counts tell little, but the fill level tells how large the agent's
  context - including what others do not see - is.

## Done when

- The token consumption of every agent in a channel, and of the channel, is summed from what is
  recorded and returned by the REST API.
- The context size of an agent's model is known to the runtime - configured or read - and Ollama
  is asked for exactly that size.
- The web UI shows per agent its fill level against the context size, and the consumption per
  agent and channel.
- It is measured and documented what `prompt_eval_count` counts with a cached prompt.
- CONFIGURATION.md (`ollama_client`), AGENT-TURN.md and WEB-UI.md say so.

## History

- **2026-10-03** — Issue created. Found: `ollama_client` records the tokens of every call in its
  message already, sets no `num_ctx`, and the `ModelProfile` knows no context size.
- **2026-10-04** — Measured with `gemma4:26b`: `prompt_eval_count` counts the whole prompt, also
  what Ollama kept from the previous call - the same prompt sent twice gave 5029 tokens both times,
  evaluated in 3142 ms and then in 53 ms; one round more gave 5042. So prompt and answer tokens
  against the context size are the fill level. `/api/show` gave the model's trained maximum as
  262144, `/api/ps` the context it was loaded with as 65536 - the server's default.
- **2026-10-04** — Decided with the maintainer:
  - **Context size:** configured as `context_size` of `ollama_client` and sent as `num_ctx`; without
    one, the size the model is loaded with, read from `/api/ps` and kept for ten minutes; unknown
    if the model is not loaded.
  - **Where it is kept:** not in `ModelProfile` - fixed when the turn begins, before the size read
    after a call is known - but in each message the model produces, as `Message::context_size`
    beside its token counts: a measurement of the call, like them.
  - **Sums:** computed from the transcript when read (`TokenUsage`), handed out per agent by the
    views; external agents unknown, not zero.
  - **Who sees it:** every member of the channel - every published message carries its token counts
    already.
  - **REST and web UI:** `GET .../participants/{aid}` returns one member with its usage, fetched
    again by the web UI when the agent's turn ends (`FullResponseGenerated`), instead of the whole
    channel. The channel's sum is no part of the API: the web UI adds up the members it shows.
  - Three steps: the client and the message, the sums through the views and REST, the web UI.
- **2026-10-04** — Step 1 done: `ollama_client` takes `context_size`, sends it as
  `options.num_ctx`, and gives every complete message its `context_size` - the configured one, or
  the one read from `/api/ps` after the call (`RunningContextSizes`, shared by the clients of a
  registry like the control tokens, kept for `RUNNING_CONTEXT_SIZE_KEPT`). A model name without a
  tag is matched as its `latest`. CONFIGURATION.md and COMPONENTS.md say so.
