# 282 — A user cannot attach an image or a file to a message

**Status:** open
**Area:** User interaction

## The problem

A message carries text only: `Message` has `content`, `reasoning`, `tool_calls` and `tool_results`,
and nothing else that a model is given (`caelumora-model/src/message.rs`). `MessageRequest`, what a
user posts over REST, is `content` and a `classification`. So a user cannot show an agent a
screenshot, a photo of a whiteboard or a diagram - although the models run here can see: Gemma 4,
Qwen-VL and others take images, and Ollama's `/api/show` says so (`capabilities: ["vision"]`).
`ollama_client` never sends one: `OllamaChatMessage` has no `images`.

What is attached belongs to the message: it was said together with the text, is classified with
it, is taken back with it, and is part of what the message's digest covers.

## Why it matters

- Much of what people want to show an agent is not text: a screenshot of an error, a scanned page,
  a chart, a mock-up. Today they have to describe it, or put it into a workspace and ask the agent
  to read it there - which a model that sees cannot do either, since no tool hands it an image.
- The same gap stands behind [208](208-generated-artifacts.md): an MCP tool that returns an image,
  or an external agent's file part, has nowhere to go that a model could be given. A type for
  attachments, chosen here, is what 208 can use for what a model is to see.

## What the model APIs accept

Checked against the official documentation and the sources on 2026-10-09. "Source" means read
from the code, not documented.

| API | Images | PDF / files | Audio | Video | In a tool result |
|---|---|---|---|---|---|
| Ollama `/api/chat` | `images: [<base64>]` on a message; bytes sniffed, no data URL, no http URL | no | WAV/MP3 bytes in `images` for models with `audio` capability (source) | no | yes, `images` on a `tool` message (source) |
| Ollama `/v1/chat/completions` | `image_url` with a data URL of jpeg, png or webp; http URLs and GIF rejected | rejected | `input_audio` | no | yes |
| OpenAI Chat Completions | `image_url` (https or data URL), `detail` | `file` part, PDF only (`file_data` or `file_id`) | `input_audio` (wav, mp3), audio models only | no | **no**: tool messages are text only |
| OpenAI Responses | `input_image` (data URL, https or `file_id`), `detail` | `input_file`: PDF as extracted text plus page images; other documents as text | no | no | yes: `function_call_output` may be a list of text, image and file parts |
| llama.cpp `llama-server` | `image_url`: base64, data URL, http, `file://` | rejected | `input_audio` (mp3, wav, flac) | `input_video` | yes (source) |
| vLLM | `image_url`, `input_image`: http, data URL | no file part | `input_audio`, `audio_url` | `video_url` | depends on the chat template |

What follows from it:

- **Every chat API is stateless:** each request carries the whole context, so every image of the
  context is sent again with every model call of every turn. Only OpenAI's Responses API keeps
  state (`previous_response_id`), and still bills what was sent before.
- **Capabilities are the model's:** Ollama's `/api/show` lists `vision` and `audio` (`image` means
  image *generation*). Its chat endpoint does not seem to refuse images for a model without
  `vision` - the client has to check itself.
- **PDFs reach only OpenAI as they are.** For Ollama, llama.cpp and vLLM a PDF has to be turned into
  text, or its pages into images, before a model is given it.
- **The common ground** of the OpenAI-compatible servers is `image_url` with a data URL of png,
  jpeg or webp, in a user message.
- **Cost:** an image takes hundreds to thousands of tokens of the context (Ollama estimates 768 per
  image when it truncates; OpenAI about 1 200 for 1024×1024 at `high`).

Sources: <https://docs.ollama.com/capabilities/vision>,
<https://github.com/ollama/ollama/blob/main/docs/api.md>,
<https://docs.ollama.com/api/openai-compatibility>,
<https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create>,
<https://developers.openai.com/api/reference/resources/responses/methods/create>,
<https://developers.openai.com/api/docs/guides/pdf-files>,
<https://developers.openai.com/api/docs/guides/images-vision>,
<https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md>,
<https://docs.vllm.ai/en/latest/features/multimodal_inputs.html>.

## Considerations

### Where the content lies

A message is cloned often - into both histories and the transcript, into every participant's copy
(issue [120](120-participant-copies.md)), into events and over SSE - and stored as JSON in the state
store. Three ways to give it an attachment:

- **A. The content in the message.** `Vec<Attachment>` whose content is a `bytes::Bytes`: a clone
  only counts a reference, so in memory nothing is copied. But a message is also *serialized*: every
  stored history, transcript and event would carry the image as base64, once per copy - megabytes
  in JSON documents that are written whole or as changes. Rejected for that.
- **B. A reference in the message, the content in a storage** (recommended). An attachment is what
  is known of it - a name, a media type, a size, the digest of its content (`digest::content`) -
  and its content lies in a storage of binary content, keyed by that digest. The message stays
  small, every clone and copy refers to the same content, and the message's digest covers the
  content through the digest of it. The price: the content has to be fetched where a model is given
  it or a user downloads it.
- **C. A reference that caches the content** (`#[serde(skip)]` beside it). Cheap where the content
  happens to be loaded, but whether it is depends on where the message came from - a state that
  cannot be told from the type. Not recommended.

### Which storage

[271](271-publishing-artifacts.md) built an `ArtifactStorage` for binary content: content by
artifact and version, a local provider, S3 to follow (272). An attachment is not an artifact: it
has no owner of its own, no scope and no versions, and lives and dies with its message - deleted
with its session, erased with it ([237](237-erasing-a-message.md)). But keeping bytes is the same
thing twice. To decide: one storage of binary content under both - a key and bytes, without
permissions - with artifacts and attachments as two users of it, or a storage of attachments of
its own. The first avoids a second provider for each backend (local, S3); it means generalizing
`ArtifactStorage` first, as a step of its own.

Keyed by the session and the content's digest, the same content attached twice in a session is
kept once, and deleting the session removes its attachments as a whole. Content is never reached by
its digest alone: a digest of known content can be computed by anyone, and asking for it would tell
whether someone attached it. It is reached through the message, by whoever may read the message.

### Upload and the REST API

- **Two steps:** the content is uploaded first (`POST …/channels/{cid}/attachments`, the raw body
  as the workspace's uploads take it), the answer is the reference; the message names the references
  it carries (`MessageRequest::attachments`). Several files, progress and a preview before sending
  come with it. What is uploaded and never sent is removed after a while. The alternative - one
  multipart request with message and files - is simpler for the server, less so for the web UI.
- **The media type is not taken from the client:** it is sniffed from the bytes and checked against
  an allow-list (png, jpeg, webp, gif; PDF once it can be given to a model). A size limit per
  attachment and a count per message, configured.
- **Download** through the message: `GET …/messages/{mid}/attachments/{n}`, as reading the message
  (`AccessQuestions`), with a test in the security suite. Served as the workspace serves files,
  `nosniff`; shown inline only for the image types of the allow-list - never SVG or HTML.

### Classification, changes, taking back

An attachment is classified as its message. Whether a modified message (issue 234) may change its
attachments, or only its text, is to be decided. A message taken back is read without its
attachments, as without its content; erasing (237) has to remove the content too, unless another
message of the session refers to the same digest.

The message's digest (`digest::message_chain`) covers the attachments - name, media type and the
content's digest - since they are part of what was said. That changes the digest: stored histories
and transcripts have to be deleted.

### Giving it to the model

- **Who fetches the content.** The context is resolved without I/O (`ContextResolver`,
  `Turn::full_context`); fetching an attachment is not. Either the `LLMClient` takes a reader of the
  storage when it is made - as a type takes the services it uses - and fetches what the request
  needs as it builds it, in the encoding of its API (base64 for Ollama, a data URL for the OpenAI
  API); or the agent fetches before it asks its model and hands the client the content beside the
  messages, which changes `LLMClient::chat_completion`. The first keeps the encoding where the API
  is known; the second keeps the client free of storage. Since every model call sends every image
  again, the client or the reader should keep recently used content.
- **What the model can take.** Whether a model sees images, hears audio or reads PDFs is a property
  of the model - `ModelProfile`, filled from `/api/show`'s `capabilities` where the backend says so,
  configurable where it does not. A model that cannot take an attachment is told of it in text (its
  name, its type, that it cannot be shown) rather than sent bytes it ignores or the request failing.
- **Converting.** A PDF is given to most models only as text or as images of its pages. A converter
  between storage and client, chosen by what the model takes, whose results are kept by the
  digest of the source. Not part of the first step; the type must leave room for it.
- **The context window.** Images fill the context fast and are sent with every call; compaction
  ([16](16-context-compression.md)) has to be able to leave old ones out, replaced by a note.
- **Other participants.** Every agent of the channel hears the message with its attachments and
  gives its model what the model can take. An external agent (A2A, file parts) is told of the
  attachment in text at first; handing on the content is a question of its own.
- **Tool results.** Images in a tool result are possible with Ollama, the Responses API, llama.cpp
  and vLLM, but not with OpenAI's Chat Completions. The type should be one that a `ToolCallResult`
  can carry later (208, MCP image content); a client for an API without it puts the image into a
  message of its own after the result.

### Web UI

A button at the message field, drag and drop and pasting from the clipboard; the attachments shown
before sending, removable; at the message, images as previews and everything else with name, type,
size and a download.

## Done when

- A user can attach images to a message in the web UI and over REST; the attachments are kept with
  the message, classified, taken back and deleted with it, and covered by its digest.
- An agent whose model can see is given the images of the messages in its context; one whose model
  cannot is told in text that an image was attached.
- Who may read a message may download its attachments, nobody else; the security suite covers it.
- Where the content lies is decided together with 271's `ArtifactStorage`, and 208 knows which
  type it uses for what a model is to see.
- CHANNELS.md, AGENT-TURN.md, STORAGE.md, REST-SERVER.md, WEB-UI.md and CONFIGURATION.md describe
  it; which stored data has to be deleted is said.

## History

- **2026-10-09** — Opened at the maintainer's request: Gemma 4 understands images, but there is no
  way to give it one. The maintainer named the questions: a type for several kinds of attachment
  that does not duplicate the content when a message is cloned, a store with only digests in the
  message as the alternative, the content to be fetched when given to Ollama, and PDFs that some
  models need converted. The section on the model APIs was researched for it.
