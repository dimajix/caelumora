# 522 — Two crates are named after the model, one for the domain and one for language models

**Status:** in progress
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

## Plan

No behaviour changes; no stored data changes. The pattern followed is the stores' backends: one
name for crate, plugin type, plugin name and section (`caelumora-store-json`, `JsonStorePlugin`,
`store_json`). Two steps, each a commit of its own; no branch but this one is open.

### Step 1: `caelumora-model` becomes `caelumora-domain`

- **The crate.** `git mv caelumora-model caelumora-domain`; its name in its manifest, in the
  members and `[workspace.dependencies]` of the root `Cargo.toml`, in every manifest that names it
  (its features with it: `caelumora-domain/test-support`), and `Cargo.lock`.
- **The code.** Every `caelumora_model::` becomes `caelumora_domain::`, in imports and in doc
  comments alike; `cargo fmt --all` re-sorts the import blocks, where the crate now sorts first.
- **"The model" for the crate.** Where prose means the crate by "the model" - "the model's
  (`caelumora_model::security`)", "a library links the API and the model", "The model derives",
  "a document of the model" -, it becomes "the domain". Found by reading each match of "model" in
  the documents, in the module docs that name the crate, in `scripts/library-deps.sh` and in the
  pre-commit hook; "the model" for a language model stays. Most of it is in AGENTS.md
  ("One owner per concern"), ARCHITECTURE.md, CRATES.md, CODING-GUIDELINES.md ("Model and
  kernel", "The model derives", the tests' documents), STORAGE.md, SECURITY.md and README.md.
- **The web UI.** The comments of `command.ts` and `mention.ts` that name what they mirror.
- **Left as they are.** The issues, and the notes of `work/`, which are records of their time.

### Step 2: `caelumora-models` becomes `caelumora-model`, `caelumora-ollama` `caelumora-model-ollama`

- **The crates.** `git mv` both; their names in the manifests, the root `Cargo.toml`,
  `Cargo.lock`, `scripts/crates.sh` (the plugins), and the graph of CRATES.md, rewritten by
  `scripts/crate-graph.sh`.
- **The names that follow them**, by the pattern above:

  | today | becomes |
  |---|---|
  | `ModelsPlugin` | `ModelPlugin` |
  | `MODELS = "models"`, name and section | `MODEL = "model"`, section `plugins.model` |
  | `OllamaPlugin`, name `"ollama"` | `OllamaModelPlugin`, name `"model_ollama"` |

  The kind of the backend stays `ollama` (`OLLAMA_BACKEND`), as the stores' backends keep theirs;
  so does the component kind `model`. `examples/config.yaml`, CONFIGURATION.md and the app's tests
  follow the section; a local configuration with `plugins.models` has to be renamed by hand, and
  the program refuses the old section as an unknown plugin's.
- **The code and prose.** Every `caelumora_models::` and `caelumora_ollama::`, and every mention
  of the two crates - in ARCHITECTURE.md, PLUGINS.md, CONFIGURATION.md, LLM-MODELS.md,
  COMPONENTS.md, AGENT-TURN.md, CRATES.md, README.md, the module docs that name them, and the
  programs.
- **The record.** CRATES.md says once, below the graph, that `caelumora-model` was the domain's
  name until this issue, so that the issues before it are read right.

### Tests

None new: the rename is checked by the build, the tests as they are, clippy, and the checks of the
pre-commit hook - `library-deps.sh` (whose check of the API names the domain), `crate-graph.sh
--check`, the builds of the crates on both levels. `grep` for the old names finds none outside the
issues, `work/` and the record in CRATES.md.

## Done when

- The three crates have their new names, and no crate, document or script names an old one, except
  the record of the rename in CRATES.md and the issues before it.
- The tests pass and `cargo clippy --workspace --all-targets` is clean.

## History

- **2026-11-02** — Opened at the maintainer's request, after issue
  [520](520-the-kernel-sees-no-model-call.md) brought `caelumora-models` beside `caelumora-model`.
- **2026-11-02** — Plan of two steps made, on the branch `issue-522-crate-names`.
- **2026-11-02** — Step 1 done: `caelumora-model` is `caelumora-domain`, and where the documents,
  the module docs and the scripts meant the crate by "the model", they say "the domain".
