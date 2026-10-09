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
  and its content lies in a storage of binary content (see *How attachments are managed*). The
  message stays small, every clone and copy refers to the same content, and the message's digest
  covers the content through the digest of it. The price: the content has to be fetched where a
  model is given it or a user downloads it.
- **C. A reference that caches the content** (`#[serde(skip)]` beside it). Cheap where the content
  happens to be loaded, but whether it is depends on where the message came from - a state that
  cannot be told from the type - if anything but one place fills it.

**Decided: B for what is stored, C for what the model is given.** Histories and transcripts hold
only the reference with the digest. The copy of a message that the model is given carries the
content beside it, `content: Option<bytes::Bytes>`, never serialized and not covered by the digest;
a clone counts a reference and copies nothing, and `LLMClient::chat_completion` keeps its signature.
Only the turn fills it (see *Giving it to the model*), on the copies it hands to the client, and a
message with content is never stored. Where it is `None`, the client tells the model of the
attachment in text.

### How attachments are managed - open

**Not by their digest.** Keeping content by its digest - kept once however often it is attached -
leaves open when an object may finally be deleted: only once nothing refers to it any more, and
references lie in every message of the same content, in the older versions of modified messages
(234), in the participants' copies (120) and in the agents' transcripts. Erasing a message (237)
would have to search all of them, and a mistake there is a leak. That is why `ArtifactStorage` keeps
content by artifact and version, not by digest (`<root>/<artifact>/<version>`), with the digest as
what is known of a version (`ArtifactVersion::digest`). Attachments keep to that: the digest is
known of an attachment and covered by its message's digest, but it is no key in a storage. Content
is shared only in memory, in the cache, by digest (see *Who fetches the content*). Nor is content
ever reached by its digest alone: a digest of known content can be computed by anyone, and asking
for it would tell whether someone attached it.

Who an attachment belongs to and how long it lives is open. Two ways:

- **A part of its message.** Kept by session and an id of its own, one object per attachment: it
  belongs to exactly one message - copies and clones of the message refer to the same id - and goes
  with its session or when its message is erased; nothing is ever counted. What is uploaded and not
  yet sent lies in an area of its own, per user, and moves under the session when the message is
  sent; what lies there too long can go without a check, since nothing refers to it. Simple, and
  what is said in a session stays in it. But an image cannot be used again in another chat without
  uploading it again.
- **A file of the user's, as an artifact.** As Open WebUI does it: what a user uploads is a file of
  theirs, not of a chat, available in general and attached to messages in any chat. Here it would be
  an artifact of the user's ([271](271-publishing-artifacts.md)) - owned, in a scope, in versions,
  its content in the `ArtifactStorage` - and a message refers to one version of it, which is fixed.
  The scope gives both lifetimes: the session's scope is the first way, the user's own scope the
  second. Deleting is then the owner's act, not a question of references. To decide with it: what
  a message shows whose artifact was deleted (a note instead of the image, its digest still
  standing), who may read the artifact - whoever may read a message it is attached to, as whoever
  may read the channel it was published in may download an artifact today -, and how an
  attachment's classification and its message's go together, when an artifact keeps the one it was
  first made with.

Either way, keeping bytes is the same thing as keeping an artifact's: one storage of binary
content under both - a key and bytes, without permissions - rather than a second provider for each
backend (local, S3, 272). With the first way that means generalizing `ArtifactStorage` first, as a
step of its own; with the second, attachments are artifacts and use it as it is.

### Upload and the REST API

- **Two steps:** the content is uploaded first (`POST …/channels/{cid}/attachments`, the raw body
  as the workspace's uploads take it), the answer is the reference; the message names the references
  it carries (`MessageRequest::attachments`). Several files, progress and a preview before sending
  come with it. What is uploaded and never sent is removed after a while. The alternative - one
  multipart request with message and files - is simpler for the server, less so for the web UI.
- **The media type is not taken from the client:** it is sniffed from the bytes and checked against
  an allow-list (png, jpeg, webp; PDF once it can be given to a model - see *Image formats*). A size
  limit per attachment and a count per message, configured.
- **Download** through the message: `GET …/messages/{mid}/attachments/{n}`, as reading the message
  (`AccessQuestions`), with a test in the security suite. Served as the workspace serves files,
  `nosniff`; shown inline only for the image types of the allow-list - never SVG or HTML.

### Classification, changes, taking back

An attachment is classified as its message - or, as an artifact of the user's, has a classification
of its own that has to fit the message's (see *How attachments are managed*). Whether a modified
message (issue 234) may change its attachments, or only its text, is to be decided. A message taken
back is read without its attachments, as without its content; erasing it (237) removes an
attachment that is part of it, while a file of the user's stays theirs to delete.

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
  again, the content cannot be fetched from the storage for every call.

  **Decided: the turn fetches, a shared cache keeps.** Not the `ContextProvider`: it gives the
  prompt parts once, at the start of a turn, while `full_context` builds the conversation anew from
  the transcript before every model call - what the provider added would be gone at the next call,
  and an attachment that comes in during the turn (a tool result's image, 208) would never be
  added. Instead:
  - The turn holds the content its context needs, by digest (`Bytes`). Before every model call
    it fetches, asynchronously, what it does not hold yet - as a rule nothing -, and `full_context`
    puts the content on the copies it hands to the client. The client only encodes it for its API.
  - The content is fetched through the owner of attachments, which keeps a **cache per process**,
    keyed by session and digest: an image several agents of a channel are given lies in memory
    once.
  - **What leaves the cache** is a question of memory, not of meaning: least recently used first,
    within a configured byte budget. A running turn holds the `Bytes` it uses itself, so the cache
    cannot take from it what it needs; what was evicted is fetched from the storage at the next
    turn again. Deleting a session and erasing a message (237) remove their content from the cache
    explicitly.
  - The client does not reach the storage, and a message with content is never stored.
- **What the model can take.** Whether a model sees images, hears audio or reads PDFs is a property
  of the model - `ModelProfile`, filled from `/api/show`'s `capabilities` where the backend says so,
  configurable where it does not. A model that cannot take an attachment is told of it in text (its
  name, its type, that it cannot be shown) rather than sent bytes it ignores or the request failing.
- **Image formats.** No API takes decoded pixels: each takes the image *file* - its bytes base64
  encoded (Ollama's `/api/chat`), or as a data URL (the OpenAI-compatible APIs) - and decodes it,
  scales it to the resolution of the model's vision encoder and cuts it into patches itself. What
  all of them take is png, jpeg and webp (Ollama documents png and jpeg and converts webp; its
  `/v1` rejects GIF; OpenAI takes non-animated GIF too). So the allow-list is png, jpeg and webp;
  GIF and HEIC (photos from iPhones) are refused at first, converting them on upload can follow.
  What was uploaded is kept as it is - its digest, its download and what the model is given are the
  same bytes. Since the backend scales anyway, a photo of many megapixels gives the model nothing a
  smaller copy would not, but costs transfer and a third more as base64; a scaled copy for sending,
  kept in the cache, is a conversion (below) and not part of the first step. The tokens an image
  costs depend on the model's encoder with Ollama, on its size and `detail` with OpenAI.
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
- How attachments are managed - as part of their message or as files of the user's - is decided,
  their content lies in the same storage as artifacts' content, and 208 knows which type it uses for
  what a model is to see.
- CHANNELS.md, AGENT-TURN.md, STORAGE.md, REST-SERVER.md, WEB-UI.md and CONFIGURATION.md describe
  it; which stored data has to be deleted is said.

## History

- **2026-10-09** — Opened at the maintainer's request: Gemma 4 understands images, but there is no
  way to give it one. The maintainer named the questions: a type for several kinds of attachment
  that does not duplicate the content when a message is cloned, a store with only digests in the
  message as the alternative, the content to be fetched when given to Ollama, and PDFs that some
  models need converted. The section on the model APIs was researched for it.
- **2026-10-09** — Decided with the maintainer, branch `issue-282-message-attachments`: what is
  stored holds only the reference with the digest; the copies the model is given carry the content,
  `Option<Bytes>`, unserialized (B with C, under *Where the content lies*). The turn, not the
  `ContextProvider`, fetches what it lacks before every model call, through a cache per process
  with a byte budget and least-recently-used eviction (*Who fetches the content*). Images are given
  as files, not decoded: png, jpeg and webp, kept as uploaded (*Image formats*).
- **2026-10-09** — Content is not kept by its digest: when an object could finally be deleted would
  be unclear, as it was for artifacts; the digest is known of an attachment, content is shared only
  in the cache. Left open at the maintainer's request: how attachments are managed - as part of
  their message, kept by session and id, or as files of the user's, as artifacts, as Open WebUI
  does it (*How attachments are managed*).
