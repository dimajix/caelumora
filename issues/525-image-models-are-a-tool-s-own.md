# 525 — An image model is reached only by a tool of its own, past the models' service

**Status:** open
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

## Done when

- Image models are backends of the models' service, configured with the language models, and the
  tool that generates images reaches them only through a client of the agent the service built.
- ComfyUI is one backend of image models, in a plugin of its own; the tool names none.

## History

- **2026-11-02** — Opened at the maintainer's request, after the discussion of the names of the
  crates (issue [522](522-two-crates-named-model.md)).
