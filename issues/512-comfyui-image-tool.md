# 512 — No tool can generate an image

**Status:** in progress
**Type:** feature
**Priority:** high
**Area:** Tools and approvals
**Summary:** No tool can generate an image, and no tool can hand its model media at all: `ToolOutput` has no media and the channel refuses a tool result that carries some.

## The problem

An agent can say text and show what it heard (`attach`, issue
[282](282-message-attachments.md)), but it cannot make an image. A ComfyUI server can - the one
beside the development machine generates images with Z-Image, Flux, Qwen-Image and SDXL, and video
and audio besides - and nothing reaches it.

The way a tool hands its model media was cut from 282 and given to issue
[208](208-generated-artifacts.md), with an MCP result's images as its first producer:
`ToolOutput` has no `media` (`caelumora-agents/src/tool/mod.rs`), so the `ToolExecutor` keeps
nothing, and the channel refuses a tool result with `media` (`ChannelLoop::require_parts`,
`caelumora-runtime/src/channel/run.rs`). An MCP server that returns images is not at hand to build
and try it with; a tool that generates images on request is.

## Why it matters

- An agent that is to illustrate, sketch or design cannot.
- 208 - MCP images, A2A file parts - waits for the way a tool hands over media, and is built and
  tried far more easily once a tool makes images on request.

## Considerations

### How a tool hands over media

Moved here from 208, as 282 decided it (*What is said and what is seen*): a tool hands over bytes,
the `ToolExecutor` stores them.

- `ToolOutput::media`, beside `value`, `classification` and `shown`: each a name, the media type if
  the tool says it, the bytes.
- The `ToolExecutor` keeps them through `AttachmentManager::create`, as the agent, classified as the
  result, into `ToolCallResult::media`. The media type is sniffed (`content::media_type`) and
  checked against the allow-list, the size against `attachments.max_bytes` - once for every tool;
  what does not pass fails the call, saying why. No tool reaches the storage.
- The channel admits `media` (refused until then, `ChannelLoop::require_parts`) and claims it as it
  claims an upload: put by the sender in this channel, not yet claimed, claimed exactly once.
- The model is given `media` along with the result as 282 gives `shown`; a model that does not see
  is told of it in text.
- The download route of an attachment (`GET …/messages/{mid}/attachments/{aid}`) serves what a
  result gave its model as it serves what one showed it; the web UI's tool call card shows `media`
  as it shows `shown` (`ToolCallView`).
- What a tool gave is numbered in the transcript as what it showed, so that the agent can `attach`
  it - the way a generated image reaches the channel's participants.

**Images only, at first.** The allow-list stays png, jpeg and webp, what the models take. The way
itself does not depend on the media type: video and audio later widen the allow-list, give the
web UI a player and a model that cannot take them a text - an issue of their own.

### The tool

A plugin of its own, `caelumora-comfyui`, after the pattern of `caelumora-mcp` (issue 325): a
component kind `comfyui_tool` offering `ToolProvider`, a remote system reached through a
`RemoteEndpoint`, errors described by `caelumora_runtime::remote`, its texts in the namespace
`comfyui`. `caelumora-app` and `caelumora-tui` list it.

**One tool, one concept.** `generate_image` makes an image from a description. Its parameters are
the tool's, not each workflow's: `prompt` (required), `workflow`, `negative_prompt`, `width`,
`height`, `seed`. A workflow is one way to do it - a model, a style, its steps - and says to which
inputs of its nodes it maps them; it may leave some unused. One tool per workflow was weighed: each
its own schema, but tools that differ in their implementation, not in what they do, and a list of
tools that grows with every style.

- **`workflow`** names one of the component's workflows; left out, the first of its list. Its
  description is a text of the catalogue, rendered with the workflows and the short description of
  each (`tool::described` with `values`); `ToolParameters` has no enum, so the names stand in the
  text. An unknown name is `invalid_arguments`, naming those there are.
- **`seed`** left out is chosen at random, so that "another one" gives another image and ComfyUI
  does not answer from its cache; the result names the seed taken, so that the model can repeat
  or vary an image.
- **`width`, `height`** left out are the workflow's; given, they are bounded by the workflow's
  `size` (below), else by a bound of the code. A value outside is `invalid_arguments`.
- The tool's name is fixed: two `comfyui_tool`s of one agent are the configuration error of two
  tools of the same name. One server is one component.

**What the tool answers:** a short text - the workflow and seed taken, the size - as `value`, and
the images of the workflow's output node as `media`.

### Where the workflows lie

A workflow is a file in ComfyUI's API format ("Export (API)"), unchanged, so that it is what
`POST /prompt` takes and can be loaded into ComfyUI again; the UI format would have to be converted
with `object_info`, as ComfyUI's frontend does, and breaks as nodes change. Beside it, a YAML file
of the same name says what the workflow is for Caelumora:

```
comfyui/workflows/
  z_image_turbo.json      # Export (API), unchanged
  z_image_turbo.yaml
```

```yaml
description: fast, photorealistic
parameters:
  prompt: 6.text          # <node id>.<input>
  negative_prompt: 7.text
  width: 13.width
  height: 13.height
  seed: 3.seed
size: { min: 256, max: 2048, multiple_of: 16 }   # optional
output: 9                 # the node whose images are the result; left out, every image
```

The defaults of the parameters are the values in the JSON. Every node and input the YAML names must
be there, of a type the parameter fits.

**The operator names the files, a personality names the workflows.** The plugin's section of the
configuration names the directory, read when the runtime is built and checked whole - a pair
missing its half, an input not there, a YAML that does not parse keep the runtime from starting,
named by the file:

```yaml
comfyui:
  workflows: comfyui/workflows    # read as `seed` is, from where the program runs
```

A component names the workflows it offers, by their names - as `prompt_texts` names a variant of
`prompts`:

```yaml
- id: comfy
  kind: comfyui_tool
  params:
    endpoint: { url: http://127.0.0.1:8188, timeout_secs: 300 }
    workflows: [z_image_turbo, flux_dev, pixel_art]
```

A name not configured keeps the component from being built. A path in `params` was rejected: a
personality is an entity its owner changes over REST, and a path there would let whoever may write
one have the server read a file of their choice and send it to an endpoint of their choice.
Workflows written into `params` whole were rejected too: kilobytes of JSON in every personality, or
in a bundle, that ComfyUI can no longer open.

### Running a workflow

- The workflow is copied, the arguments written into the inputs the YAML names, and sent with
  `POST /prompt`; ComfyUI answers with a `prompt_id`.
- The tool asks `GET /history/{prompt_id}` at short intervals until the prompt is done or failed,
  rather than following the WebSocket `/ws`: no further dependency, and a tool cannot report
  progress anyway. Then it fetches the output node's images with `GET /view`.
- The endpoint's `timeout_secs` bounds the whole call - the queue and the run -, 300 s by default;
  each request is bounded by the code besides.
- **A call dropped - the turn cancelled or stopped, the timeout over - ends its prompt in ComfyUI**,
  in a task of its own, bounded and logged, that the drop does not await: removed from the queue
  (`POST /queue` with `delete`) where it waits, interrupted where it runs (`POST /interrupt` with
  the `prompt_id`, so that no other prompt is hit - to be checked against ComfyUI 0.32). Otherwise
  a call given up would hold the GPU for minutes. It is the second exception, after
  `terminal_tool`, to "a tool binds whatever it starts to its own future"; TOOLS.md states the
  exception in general: a tool whose work lives outside the process ends it on drop, in a bounded
  task of its own.
- A failed prompt (ComfyUI's `status` with an error, a node that raised) is `failed`, with what
  ComfyUI said of the node, never the endpoint.

### Approval and confidentiality

- **`requires_approval`**, as `mcp_tool`'s (`ApprovalMode`), `false` by default: the tool changes
  nothing but the GPU's time, and what leaves is the prompt, which `destination` governs.
- **`destination`**, as `mcp_tool`'s, `public` by default: the server's clearance is the
  operator's to configure. ComfyUI keeps the prompt in its history and, with `SaveImage`, in the
  metadata of the PNG in its output directory - the clearance covers what stays there.
- **The result is classified as the turn's label**, as the built-in tools whose answer is derived
  from the arguments alone: the image follows from the prompt. No `classification` parameter, since
  nothing needs it.
- `description` as `mcp_tool`'s: what the tool set is for; left out, the catalogue's text.

### What follows

- **`edit_image`** - image to image, inpainting, upscaling, later image to video: a second tool of
  the same mechanism, whose `image` is the number of an attachment (`a3`), uploaded with
  `POST /upload/image`. The image leaves the channel for ComfyUI, so its classification has to be
  allowed at the `destination`, not the arguments' alone. An issue of its own.
- **Video and audio**: the allow-list, a player in the web UI, a text for a model that cannot take
  them. An issue of its own.

## Done when

- A tool can hand its model media through `ToolOutput::media`; the executor keeps them as the
  agent, classified as the result; the channel admits and claims them; the model is given them,
  the web UI shows them at the call, and the agent can `attach` them. The security suite covers
  that a user who does not see the result cannot download its media.
- `comfyui_tool` offers `generate_image` over the workflows the configuration names; a call
  generates an image in ComfyUI and gives it the model; a call dropped ends its prompt there.
- The example configuration names an example workflow (`examples/comfyui/workflows`), which an
  example personality offers.
- TOOLS.md (how a tool hands over media, the tool, the exception on drop), COMPONENTS.md,
  CONFIGURATION.md, PLUGINS.md, ARCHITECTURE.md (the plugin) and, for the card, WEB-UI.md describe
  it; 208's history says that how a tool hands over media moved here.

## Plan

1. **How a tool hands over media**, in the core, with a test tool of `test-support` as its
   producer: `ToolOutput::media`, the executor, `require_parts`, the model's copy, the download
   route, the web UI's card. 208's history records the move.
2. **The workflows**: the plugin `caelumora-comfyui`, its section, reading and checking the
   directory.
3. **The tool**: `comfyui_tool` and `generate_image` - the client, the run, the drop.
4. **Example and documents.**

## History

- **2026-10-30** — Issue created, before 208 and taking from it how a tool hands over media, with
  ComfyUI as its first producer. Decided with the maintainer: images only; one tool,
  `generate_image`, with a workflow as an optional parameter, the first by default; workflows as
  files of ComfyUI's API format with a YAML beside each, in a directory the configuration names,
  offered by name; a random seed, sizes bounded per workflow; a call dropped ends its prompt in
  ComfyUI; no approval, destination public, the result classified as the turn's label. `edit_image`
  and video follow as issues of their own.
- **2026-10-30** - Step 1 done: how a tool hands over media. `ToolOutput::media`, each a
  `ToolMedia` of a name and its bytes (`caelumora_model::tool`, so that the executor hands the same
  type on to the manager), without a media type - the bytes tell it, as for an upload
  (`content::media_type`); a type the tool says is to come with the first producer whose bytes do
  not tell it, to refine what they tell, never to contradict it. The `ToolExecutor` has them kept
  for a result not withheld through `ActorRuntime::create_media` and
  `AttachmentManager::create_media`: in the agent's name, classified as the result, images alone
  (`content::is_image`), at most `attachments.max_per_message`, all of them or none - what was kept
  before one failed is removed again; anything else fails the call. Decided with the maintainer
  after the review (option A of three): what a tool gave waits for no message, unlike an upload -
  the result refers to it from the moment it is kept, and the turn that holds the result keeps it
  across a restart and however long the other calls of its step wait for approval, which an upload
  waiting in memory would not survive. The channel admits it as it admits what a result showed, by
  `lies_in` and its label; `AttachmentManager::claim` stays as it was. Rejected: storing the
  waiting uploads (B, a stored state of its own), and the channel dropping media it cannot claim
  (C, which leaves the turn naming content that is gone). `Message::carried` yields the media, so
  that loading for the model, the numbers, the download and the channel's flow take them as they
  take `shown`; `ModelProfile::frame` notes them after the result's text - number, name, type,
  whether given along - so that a model that cannot see learns of them and any model can `attach`
  them; `ollama_client` sends them with the result. No test tool of `test-support` was needed: the
  executor's tests bring their own provider, as for `shown`.
