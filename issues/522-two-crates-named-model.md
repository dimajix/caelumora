# 522 — Two crates are named after the model, one for the domain and one for language models

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** The crate of the domain's data and rules is named `caelumora-model` and the plugin of the language models `caelumora-models`, so that two crates a letter apart mean two unrelated things, and "model" means the domain in one and a language model in the other.

## The problem

Since issue [520](520-the-kernel-sees-no-model-call.md) the workspace has two crates a letter apart:

- **`caelumora-model`** holds the data and rules of the domain: actors, sessions, channels,
  messages, projects, what is stored (AGENTS.md, "One owner per concern"). It is linked by every
  crate, 344 files name it.
- **`caelumora-models`** is the plugin that offers the language models as a service, with the
  backend trait `ModelProvider` and the component kind `model`; `caelumora-ollama` brings one of
  its backends.

"Model" means the domain in the first and a generative model in the second. In the AI field, the
second meaning is the established one: Ollama, OpenAI and Hugging Face all speak of models, an
image generator such as Stable Diffusion included. The clash comes from the generic name of the
domain crate, not from the word for a language model.

## Decided

Discussed with the maintainer on 2026-11-02; terms such as "generator", "genmodel", "inference" and
"llm" were weighed and set aside: "generator" is taken in programming (iterators, coroutines), the
others are made up or exclude images.

| today | becomes |
|---|---|
| `caelumora-model` | `caelumora-domain` |
| `caelumora-models` | `caelumora-model` |
| `caelumora-ollama` | `caelumora-model-ollama` |

`caelumora-model-ollama` follows the scheme of the stores' backends (`caelumora-store-json`,
`caelumora-store-postgres`): the service's crate, then the backend.

## Considerations

- **A name used again.** `caelumora-model` will name another crate than it did. The issues and
  commits before this one mean the domain by it, and their text is not rewritten (issues/README.md);
  a reader of an old issue has to know of the rename. CRATES.md names the rename once, with this
  issue's number.
- **Two commits.** The domain is renamed first, the models' plugin into the name freed second, each
  a commit of its own, so that each reads as a rename: a commit that does both at once shows
  `caelumora-model` both removed and added. A branch made before the rename and merged after it
  fails to build where it names `caelumora_model`, with errors of items missing from the models'
  plugin rather than of a crate not found; whoever merges it renames its imports first.
- **The names in the crates.** With the crates the names that follow them: `ModelsPlugin` becomes
  `ModelPlugin`, `OllamaPlugin` `OllamaModelPlugin` (as `JsonStorePlugin`). The section of the
  configuration, `plugins.models`, holds several named models and may keep its plural; to be
  decided with the step.
- **When.** The rename touches nearly every file of the workspace; every branch open at the time
  gets a conflict in most of what it changes, as the merge of issue 517 showed. It is done when
  few branches are open, and nothing else is done in it.
- **What it touches.** The manifests and `Cargo.lock`, every `use caelumora_model::…`, AGENTS.md
  (the owners of "One owner per concern"), ARCHITECTURE.md, CRATES.md (graph and rules),
  PLUGINS.md, CONFIGURATION.md, LLM-MODELS.md, COMPONENTS.md, AGENT-TURN.md, README.md,
  `scripts/crates.sh`, the checks of the pre-commit hook that name crates, and the programs
  (`caelumora-app`, `caelumora-tui`). The issues are not changed.

## Done when

- The three crates have their new names, and no crate, document or script names an old one, except
  the record of the rename in CRATES.md and the issues before it.
- The tests pass and `cargo clippy --workspace --all-targets` is clean.

## History

- **2026-11-02** — Opened at the maintainer's request, after issue
  [520](520-the-kernel-sees-no-model-call.md) brought `caelumora-models` beside `caelumora-model`.
