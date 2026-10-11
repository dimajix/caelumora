# Language Models

Which settings of `ollama_client` suit which model, as far as it was measured. The parameters
themselves are described in [CONFIGURATION.md](CONFIGURATION.md#the-model); the rules they set, in
[AGENT-TURN.md](AGENT-TURN.md). What is not measured is said so: a setting without a measurement is
the default, not a recommendation.

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
| `context_size` | as much as the machine holds, e.g. `65536` | The model was trained for 262144 tokens; unless told, Ollama loads it with its own default, which may be far fewer, and cuts a longer prompt off without saying so. Set, the gauge in the web UI knows the size |
| `think` | left out | Ollama's default for the model; not measured |
| `reasoning_history` | `current_turn` (default) | What reasoning models document; not measured for gemma4 |
| `system_messages` | `only_at_start` (default) | Holds for every model. Whether gemma4 keeps a later system message in place is not measured: Ollama renders its chat template internally |

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
(`channel_members_prompt_provider`); without the explanation some imitate the marker in their
answers. Whatever the format, a marker is a claim: a participant can write another's into their own
message, and the models measured believe it (findings 8 and 12-18). Nothing with
consequences may rest on whom the model believes said something.

## External agents

An `a2a_agent` takes the same `sender_format` and `sender_style`. The model behind a remote agent is
unknown, so nothing can be recommended from measurements; the default (`prefix`, `handle`) is what
most models above handle reasonably well.
