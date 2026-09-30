# 130 — The language model is configured through the environment, not the configuration

**Status:** done
**Area:** Observability and operations

## The problem

Every section of the configuration is in `AppConfig`, except the one thing
every agent needs: the language model. `OllamaClient::from_env` reads
`OLLAMA_BASE_URL`, `OLLAMA_MODEL` and `OLLAMA_THINK` from the environment,
and both binaries make it with `OllamaClient::default()`. Found while
writing `docs/CONFIGURATION.md` ([129](129-documentation-structure.md)).

## Why it matters

- A deployment is described by its configuration file - except for the
  model, which lives in the process environment instead.
- There is one model for all agents; a personality cannot use another one,
  and there is no place for a second backend or for credentials (which would
  belong in the secret chain).
- A misspelt variable silently falls back to its default, unlike a misspelt
  key in the configuration (issue 96).

## Considerations

- **An `llm` section in `AppConfig`**, tagged by backend (`type: ollama`,
  `base_url`, `model`, `think`), with credentials named as secrets like
  `mcp_tool`'s `auth.secret`. The environment variables could stay as a
  fallback during a transition, or go.
- **Several named models**, which a personality refers to by name. Larger;
  only worth it once a second backend or per-personality models are wanted.

## Done when

- The model is configured in the configuration file, with its default
  documented in `docs/CONFIGURATION.md`.

## History

- **2026-09-30** — Opened, found while documenting the configuration (129).
- **2026-09-30** — Decided with the maintainer: neither an `llm` section nor
  named models, but the model as a component of the agent. Done:
  `LLMClient` is a facet of which an agent has exactly one
  (`ComponentRegistry::single`), like the `ContextProvider` and the
  `ToolExecutor`; the agent takes it with `components.one`, so a
  personality may configure its own and `component_defaults` sets it for
  every other agent. The default kind is `ollama_client`
  (`component_impl/ollama_client.rs`, formerly `llm_impl/ollama.rs`) with
  the parameters `base_url`, `model` and `think`, whose defaults are the
  former ones; unknown parameters are refused. The environment variables
  are gone. The `RuntimeManager` no longer holds a model and hands none to
  its actors (`ActorRuntime::llm_client` is gone); instead it is given the
  `ComponentRegistry` (`ComponentRegistry::built_in`), in which a test
  registers a stand-in for the model as a kind of its own.
