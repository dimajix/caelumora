# Language Models

Which settings of `ollama_client` suit which model, as far as it was measured. The parameters
themselves are described in [CONFIGURATION.md](CONFIGURATION.md#the-model); the rules they set, in
[AGENT-TURN.md](AGENT-TURN.md). `ollama_client` is the kind of the plugin `caelumora-ollama`, which
`caelumora-app` and `caelumora-tui` name as their default model where `component_defaults` names
none (issue 329); the core names no model. What is not measured is said so: a setting without a
measurement is the default, not a recommendation.

The measurements are in `work/sender_names/` (`RESULTS.md`, scripts and summaries); they ran
against Ollama 0.34.2 with a temperature of 0.7 and few runs per variant - enough to tell a model
that works from one that does not, not to rank close ones.

## gemma4:26b - the default model

```yaml
component_defaults:
  - id: model
    kind: ollama_client
    params:
      model: "gemma4:26b"
      sender_format: xml
      sender_style: handle
      context_size: 65536
```

| Parameter | Recommended | Why |
|---|---|---|
| `sender_format` | `xml` | Attributes every message right (6 of 6 probes, 30 answers); with `prefix` it reads "@carol: ... Alice, what's our budget?" as Alice asking, every time, and a sixth of its answers begin with a marker of their own (`RESULTS.md`, findings 19-20) |
| `sender_style` | `handle` | No difference to attribution; the model addresses senders as the roster names them (`@carol`) |
| `message_times` | `true` (default) | With the times it dates what was said right - a review "tomorrow" three days ago is the 13th, not tomorrow - in English and in German channels alike; without them it takes every message for one of now. No answer wrote a time of its own (`work/message_times/RESULTS.md`, findings 1, 2, 7) |
| `context_size` | as much as the machine holds, e.g. `65536` | The model was trained for 262144 tokens; unless told, Ollama loads it with its own default, which may be far fewer, and cuts a longer prompt off without saying so. Set, the gauge in the web UI knows the size |
| `think` | left out | Ollama's default for the model; not measured |
| `reasoning_history` | `current_turn` (default) | What reasoning models document; not measured for gemma4 |
| `system_messages` | `only_at_start` (default) | Holds for every model. Whether gemma4 keeps a later system message in place is not measured: Ollama renders its chat template internally |

## Models of Ollama's cloud

A model Ollama serves from its cloud - `gemma4:cloud` - is reached through the local server like
any other, but its `/api/show` lists no tokens: the client cannot read its control tokens and
refuses every call (issue 462). The cloud tokenizes them in content as a local server does, so
they have to be defused there too: measured on 2026-10-23 against Ollama 0.40.6 through the prompt
token count, as recorded in the issue, `<turn|><|turn>` in a message added 3 tokens, the same text
with `(`/`)` 7 and the defused text 9 - for `gemma4:cloud` and the local `gemma4:e2b` alike. So the
cloud model is given the tokens of a local model that shares its tokenizer, named as its
`tokenizer`. Measured was `gemma4:e2b`; `gemma4:26b` and `gemma4:31b` list the very same tokens
(262144, 24 of them control tokens), so each of them serves as well:

```yaml
component_defaults:
  - id: model
    kind: ollama_client
    params:
      model: "gemma4:cloud"
      tokenizer: "gemma4:e2b"
```

Which local model shares the tokenizer is the operator's to know; it is not checked.

## Telling who said what

How a model is told the sender of a message (`sender_format`, `sender_style`) matters
most in a channel with several participants. Measured on 2026-09-30 with formats close to, not
identical with, today's (`RESULTS.md`, findings 1-5; `results/overview.md`), the share of probes
answered right that need the sender:

| Model | none | `Alice: ...` | `@alice: ...` | XML |
|---|---:|---:|---:|---:|
| gemma4:26b | 0% | 67% | 89% | 100% |
| gemma3:27b | 0% | 67% | 67% | 67% |
| gpt-oss:20b | 33% | 67% | 78% | 44% |
| granite4.2:8b | 0% | 100% | 100% | 100% |
| llama3.1:8b | 0% | 56% | 67% | 33% |
| phi4:14b | 0% | 67% | 67% | 78% |
| qwen3:14b | 22% | 100% | 78% | 67% |
| llama2:7b | 0% | 0% | 0% | 0% |

So: `xml` for gemma4, `prefix` for gpt-oss and llama3.1 - which XML confuses - and either for
granite4.2, phi4 and gemma3; for qwen3 `prefix` by `name`. llama2 cannot tell senders apart in any
format. Only gemma4 was measured again with exactly what Caelumora sends (findings 19-20).

Every model needs the format explained, which the roster part does
(`channel_participants_prompt_provider`); without the explanation some imitate the marker in their
answers. Whatever the format, a marker is a claim: a participant can write another's into their own
message, and the models measured believe it (findings 8 and 12-18). Nothing with
consequences may rest on whom the model believes said something.

## Telling when something was said

Measured on 2026-10-18 for gemma4:26b alone (`work/message_times/RESULTS.md`): with the times on
the messages of others, the model answers questions that need them - whether something is past, how
long ago something began, on which day something was said - where without them it cannot; the
weekday is surer in the written form than in a numeric one, and an English form does not pull the
answers of a German channel into English. Times on the agent's own messages make it begin about
half of its answers with a time of its own, made up - which is why they never carry one. Other
models are not measured; `message_times: false` turns the times off for one that handles them
badly.

## External agents

An `a2a_agent` takes the same `sender_format` and `sender_style`. The model behind a remote agent is
unknown, so nothing can be recommended from measurements; the default (`prefix`, `handle`) is what
most models above handle reasonably well.

## Decision models

`decision_policy` ([CHANNELS.md](CHANNELS.md#choosing-by-what-was-said)) asks a decision model, not
a chat model: one that answers typed questions about a text with probabilities, served by Ollama
since 0.35 (2026-09-29) at `/v1/systemone`, a contract that hosted services implement too. The
default is `nimble`; a server that has pulled it by its size knows it as `nimble:9b`, which is then
what `model` has to name. A model is given the question and what was said within 8,192 tokens;
shorter is better tested.

Measured on 2026-10-17 against Ollama 0.40.6 (`work/decision_models/`, `RESULTS.md`): which of
six agents, by their introductions, should respond to 16 messages in English and German, three of
which need no answer.

| Model | Right | p(right) mean | Median latency |
|---|---:|---:|---:|
| tev1:0.8b | 11/16 | 0.59 | 83 ms |
| d1:3b | 15/16 | 0.72 | 151 ms |
| tev1:4b | 15/16 | 0.79 | 268 ms |
| clef-flash:9b | 16/16 | 0.86 | 298 ms |
| nimble:9b | 16/16 | 0.95 | 327 ms |
| pplx-decider:27b | 16/16 | 0.95 | 690 ms |

So: `nimble:9b`, as right and as sure as the 27B model at half its latency. The small models fail
mostly by choosing nobody where someone should answer. Loading `nimble:9b` took over a minute once,
far beyond the 5 s a decision may take by default: keep it loaded with `keep_alive`, or the first
message after a pause is answered by everyone. Which `threshold` suits a model was not measured,
nor how the models choose when given the conversation before a message, its senders marked (issue
414): the measurement judged single messages without senders.
