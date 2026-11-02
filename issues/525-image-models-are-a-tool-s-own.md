# 525 — An image model is reached only by a tool of its own, past the models' service

**Status:** in progress
**Type:** refactoring
**Priority:** low
**Area:** Integrations
**Summary:** The images an agent generates come from the tool `comfyui_tool`, which reaches its ComfyUI server itself, so that image models are configured, reached and in future accounted for apart from the language models, and a second image backend would bring a tool of its own.

## The problem

Since issue [520](520-the-kernel-sees-no-model-call.md) the language models are a service of the
plugin `caelumora-models`: backends (`ModelProvider`) configured under `plugins.models`, a client
per agent (`ModelClient`) that is given the agent's `Acting`, so that the service can later account
for every call. Images are generated otherwise (issue [512](512-comfyui-image-tool.md)): the library
`caelumora-comfyui` brings the component kind `comfyui_tool`, whose tool `generate_image` posts a
workflow to the ComfyUI server its component names, itself.

So an image model is not a model of the service: it is configured on the tool's component
(endpoint, workflows) and in the section `plugins.comfyui`, it is reached past the service and will
not be accounted for with it, and the tool is bound to ComfyUI - another backend of images would
bring a tool of its own, with the same parameters.

## Considerations

- **The same cut as the language models.** A backend trait of image models in the models' plugin
  (after issue [522](522-two-crates-named-model.md) `caelumora-model`), say `ImageModelProvider`,
  beside `ModelProvider`; a client of an agent as a facet in `caelumora-api`, say `ImageClient`,
  built by the plugin per agent with its `Acting`, as `ModelClientFor` is. The tool stays on the
  agent side, a library's, and links only the API: a library cannot link a plugin (issue
  [516](516-the-runtime-trusts-the-agents-linked-into-it.md)), so the trait the tool calls has to
  be the API's, not the plugin's.
- **What a request is.** The parameters are already ComfyUI-independent: `prompt`,
  `negative_prompt`, `width`, `height`, `seed` (`caelumora-comfyui/src/workflow.rs`, `Arguments`).
  A request of these, answered with images, is a generic interface; where they go in a graph, and
  the size bounds, stay the backend's.
- **A workflow is a model.** A workflow is what an image model is to the tool: named, described for
  the agent, with bounds of its own. Each could be a named image model of the service - backend
  `comfyui`, with its endpoint and workflow -, and the tool names the image models it offers rather
  than workflows. The directory of workflows (`plugins.comfyui.workflows`) stays the backend's.
- **Crates.** The backend becomes a plugin, `caelumora-model-comfyui` (as `caelumora-model-ollama`
  after 522); the tool, free of ComfyUI, a generic kind - in `caelumora-agents` among the built-in
  kinds, or a small library of its own. Which is to be decided.
- **Names beside each other.** With images beside language, `ModelClient`, `ModelProvider` and the
  kind `model` say less than they did; whether they become `LanguageModel…` or keep their names as
  the default kind of model is to be decided with the interface.
- **Behaviour.** What an agent sees and does stays the same; the configuration changes (the image
  models under the models' section, the tool's component naming them), and the example and
  CONFIGURATION.md with it.

Depends on [522](522-two-crates-named-model.md). The interface - trait, facet, request, how a tool
names its models - is put before the maintainer first.

## Decision

Decided with the maintainer on 2026-11-02, in three steps, each reviewed on its own:

1.  **The language model is named so.** `ModelClient` becomes `LanguageModelClient`,
    `ModelProvider` `LanguageModelProvider`, `ModelClientFor` `LanguageModelClientFor`, the kind
    `model` `language_model`, `with_unasked_model` `with_unasked_language_model`; in
    `caelumora-model` the module of the kind, `component`, becomes `language_model`, with
    `LanguageModelParams` and `LANGUAGE_MODEL_KIND`, and `ModelManager::client_for`
    `language_client_for`, beside which step 3 puts `image_client_for`. What both kinds of model
    share keeps its name: `ModelError` and `ModelResult` - the failure of a model's backend -,
    `ModelManager`, the crate and plugin `caelumora-model` and the module `caelumora_api::model` -
    the models the platform offers. `ModelProfile` and `ModelProfileProvider` keep theirs too: the
    profile is how an agent's context is written, which an external agent provides as well, without
    a client of a language model. A rename alone, without a change of behaviour.
2.  **The section is split.** `plugins.model` holds the language models under `language` and the
    image models under `image`, one plugin with one section; `with_default_model` writes under
    `language`.
3.  **Image models are the service's.**
    *   The API (`caelumora_api::image`) gives an agent `ImageModelClient` - its `name`, its
        `description`, and `generate(initiator, &ImageRequest) -> ModelResult<Vec<Image>>` -, an
        `ImageRequest` of `prompt`, `negative_prompt`, `width`, `height` and `seed`, and an `Image`
        of a name and its bytes. `ModelError` gains `Unfit`, a request outside what the model
        takes, which the tool gives the model to correct.
    *   `caelumora-model` brings the backend trait `ImageModelProvider` (`description`,
        `generate`) and the kind `image_model`, which names one image model (`name`, the one there
        is where left out) and provides its `ImageModelClient` for the agent's `Acting`, as
        `language_model` does. `ImageModelClient` is no single: an agent may have several.
    *   ComfyUI is the backend `comfyui` of the plugin `caelumora-model-comfyui`, with the
        parameters `endpoint` and `workflow`, the path of a workflow's pair of files; one image
        model is one workflow, and there is no directory of workflows any more.
    *   The tool is the kind `image_tool` of the library `caelumora-tool-image`, which needs every
        `ImageModelClient` of its agent and offers them by name, the first the default; the
        argument `workflow` becomes `model`. It keeps `requires_approval`, `destination` and
        `description`. `caelumora-comfyui` is removed.
    *   `destination` stays the tool's for now: it describes where the server keeps the arguments,
        which is the model's, but a tool of several models would need the meet of their
        classifications, or the service a check of each call - a question of its own.

The crate of the workspace's tools is to be named after the scheme of `caelumora-tool-image`,
issue [531](531-workspace-tools-named-apart-from-the-tools.md).

## Done when

- Image models are backends of the models' service, configured with the language models, and the
  tool that generates images reaches them only through a client of the agent the service built.
- ComfyUI is one backend of image models, in a plugin of its own; the tool names none.

## History

- **2026-11-02** — Opened at the maintainer's request, after the discussion of the names of the
  crates (issue [522](522-two-crates-named-model.md)).
- **2026-11-02** — Plan of three steps decided with the maintainer (see Decision), on the branch
  `issue-525-image-models`.
- **2026-11-02** — Step 1 done: the language model's facet, backend trait and kind are named
  `LanguageModelClient`, `LanguageModelProvider` and `language_model`. Stored personalities and
  configurations whose components name the kind `model` have to name `language_model`.
- **2026-11-02** — Step 2 done: `plugins.model` holds the language models under `language`, read
  as a section that refuses a key it does not know; `with_default_model` is
  `with_default_language_model` and fills `language`. A configuration that names its language
  models directly under `plugins.model` has to move them under `language`.
