# 282 — A user cannot attach an image or a file to a message

**Status:** open
**Type:** feature
**Priority:** high
**Area:** User interaction
**Summary:** A user cannot attach an image or a file to a message, so a model that can see, such as Gemma 4, is never shown one.

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

### How attachments are managed

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

Since then artifacts have become a plugin of their own (`caelumora-artifacts`, issue 328), while
`Message` lies in the core: as artifacts of the user's, attachments would make the core depend on
a plugin.

**Decided: an attachment is part of its message; where its content comes from is not.** A
recorded copy of what was attached has to be kept for audit as long as its message, unchanged -
so a message cannot borrow its content from something its owner may delete, replace or move. An
attachment names what is known of it - its name, media type, size and the digest of its content -
and where its content lies: the name of a binary storage, as configured, and a key in it. The key
is opaque, given by whoever puts the content there; the digest is no key, but what the content is
checked against, so that the message records what was said wherever its bytes lie. A storage knows
keys and bytes and nothing of permissions: who may read a message may read its attachments, and
the reference points to bytes, never to an entity with permissions of its own, such as an
artifact's version. Under a key a message refers to, only its message removes the content.

- **The binary storages move into the core.** They leave `artifacts.storage` for a section of the
  core's, and artifacts use them as attachments do, in a key space of their own - as
  `Artifact::storage` already names the storage its content lies in. A step of its own, before
  attachments, without a change of behaviour.
- **Content from elsewhere is copied.** A version of an artifact - later a library of uploads, or a
  cloud drive - is attached by copying it within the storage (`copy(from, to)`) to a key of the
  message: two lifetimes, nothing counted. A backend makes the copy cheap where it can - a hard
  link or reflink on a local disk, `CopyObject` in S3; between two storages it is a transfer, slower
  but not wrong. Whether and when the artifacts plugin attaches a version - `publish_artifact`
  included - is the plugin's to decide; neither the message nor the core change for it.
- **What a user attaches directly is not versioned.** It is uploaded first into an area of the
  user's own, readable only by them, and becomes the message's attachment when the message is
  sent. A library of uploads to manage, as Open WebUI has, would be another source to copy from,
  not part of this.
- **Who removes which keys:** the core those of attachments - when their message is erased (237)
  or its session purged - and those of uploads never sent, after a while; the artifacts plugin
  those of artifacts, when one is purged. The storage knows nothing of it.

**Decided: an attachment lies in its channel's scope.** A message is said in one channel, and its
attachment is part of what was said there; a channel is deleted, restored and purged on its own
within its session (`SessionLoop::delete_channel`), and the agents' transcripts lie in it as well
(`AgentChannelState`). So attachments go as the rule for scopes says - what lies in a channel's
scope is used only within it and goes with it (STORAGE.md) -, not with the session, which would
leave those of a purged channel behind with nothing referring to them. "Owner" here means the
lifetime, not `SecurityAttributes::owner_id`: who reads an attachment is still its message's to
say.

- **The global name is the `AttachmentId`**, given when the content is uploaded and kept when the
  message is sent; every copy of the message, the REST API and the web UI name the attachment by
  it. Its key in the storage is `<channel>/<attachment id>`: the id makes it unique, the channel
  before it lets a purge remove a channel's attachments by the prefix, as `ArtifactStorage::remove`
  removes an artifact's versions - no message has to be enumerated. Erasing a message (237)
  removes single keys.
- **Not yet sent is a record, not a place.** The key is fixed at the upload, so sending moves
  nothing; what marks an upload as not yet sent is a record of who uploaded it and when, which no
  message names yet - by which only that user reads it, and it expires after a while, at the
  latest with its channel. Where that record is kept is for the plan.
- **Deleting and restoring a channel** hides its attachments and brings them back, with nothing
  marked at them: without its channel an attachment is not reached. Purging it - also through its
  session - removes them, joined as artifacts and workspaces are (`…_lying_in`).

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

**Decided: an attachment has its message's classification, and none before it is sent.**

- **An upload not yet sent has no label.** Only the user who uploaded it reads it, so nothing flows
  anywhere - as with text in the message field, not yet sent. What it is uploaded into is checked
  all the same: the storage attachments are kept in is configured (as `artifacts.publish_to` names
  artifacts'), and an upload is refused unless the channel's classification may be put into that
  storage's `destination` (`may_publish`), as making a workspace in a storage is checked. Since a
  message is never classified above its channel, whatever label the user then gives the message
  fits the storage. One storage for attachments, not one per label, as long as nobody needs more.
- **A sent attachment has no label of its own.** Who may read a message reads its attachments, so
  a label of their own would change nothing: a higher one would need the message classified so, a
  lower one matters only where another channel is read, and there it follows its message. A channel
  lowered fits its messages (`Classification::fitted_into`), and their attachments and renditions
  follow without anything stored at them. A message lowered is not checked further; a message
  raised is checked against the `destination` of its attachments' storage besides, and refused if
  its new label may not be put there - as raising a workspace is.
- **Attaching an artifact's version brings its label into the message.** For a user: they have to
  be allowed to read the artifact, and its classification has to flow into the message's
  (`may_publish`); if it does not, attaching is refused, and the message's label is not raised -
  the API does not reinterpret what it is told. The web UI may propose the label needed. For an
  agent it follows from its turn: having read the artifact, the turn's label covers the artifact's.
  The artifacts plugin checks it in its copy; the core is handed a copy only. Kept the other way -
  an attachment kept as an artifact - the artifact takes the message's classification as it is then.
- **Nothing reads the content to classify it.** The user classifies what they attach as they
  classify what they write.

**Decided: a modification may remove and add attachments; only what stands is read.** A history
keeps every version (234): the message said stays as `Said`, each modification is appended as
`Modified` with the whole new version, a taking back as `Revoked`, all of it in the digest chain;
reading folds the changes into what stands (`HistoryEntry::fold_into`), and nobody is given an
older version - neither a page of the history nor a context. `Message::modify` takes over content,
reasoning, classification and mentions, and leaves tool calls and results as they were said.

- **An older version keeps its attachments, as a record.** Their keys stay as long as the history
  refers to them: a modification that removes an attachment removes nothing from the storage - only
  erasing the message (237) or purging its session does.
- **Only what stands is read and downloaded,** as only what stands of the text is. An attachment
  removed is not found, by an old link neither.
- **An attachment has an id of its own** (`AttachmentId`), stable within its message: a download
  is `GET …/messages/{mid}/attachments/{aid}`, not by the attachment's place, which a removal
  shifts; and the owner of attachments makes their keys from the channel and that id (see *How
  attachments are managed*).
- **`MessageRequest::attachments` names the attachments as they are to stand,** read alike for a
  new message and a modification: the id of an attachment it has keeps it, the reference to an
  upload not yet sent adds it, one not named is removed. `Message::modify` takes the attachments
  over as it takes the content; `Modified` carrying the whole version, the digest chain covers them.
  The main case: a wrong screenshot, which from the next turn on no agent is given - taken back,
  not erased, as text is.
- **A tool result's attachments (208) stay with their result,** which `modify` leaves as it was.
- **Taking a message back** empties its attachments as its content; their keys stay until it is
  erased.
- **A modification that raises the label** is checked against its attachments' storage, as a
  reclassification is (above).
- An agent's transcript keeps which rendition its model was given of an attachment since removed:
  what it saw stays recorded; its next turns are given what stands.

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

  **Decided: the original is attached, a rendition belongs to its attachment.** Which rendition a
  model is given depends on the model, not on the message - markdown for one, images of the pages
  for another, the PDF itself for OpenAI -, and a message, once stored, does not change. So the
  attachment holds the original, as uploaded. A rendition of it in a target format lies in the same
  storage, under a key made of the attachment's and the target format; it is made once - a request
  while a conversion is under way waits for that one - and removed with its attachment. Every copy
  of the message refers to the same attachment, so the agents of a channel share its renditions.
  Not kept by the source's digest across sessions: how fast a conversion is answered would tell
  whether someone else attached the same document, and a converter that is not deterministic (OCR,
  a model) makes a rendition a record of what a model saw, not a cache that can be made again. The
  price: the same document in two messages is converted twice. The owner of attachments converts;
  the converters are interchangeable.

  The transcript does not hold a rendition, only which one the model was given - its target
  format, the converter and its version, the rendition's digest -, so that what a model saw can be
  checked, and a model changed in between is given the rendition it takes at its next turn.

  **Deferred: no central conversion in the first step.** The decision above stands, but is built
  only once something needs it - a channel without a workspace, a model that cannot handle the
  tools. Until then a model works on what needs converting itself, on a copy in a workspace (see
  *What a model is given in the first step*).
- **The context window.** Images fill the context fast and are sent with every call; compaction
  ([16](16-context-compression.md)) has to be able to leave old ones out, replaced by a note.

  **Decided: a large document is read through a tool.** A rendition below a threshold is put into
  the context; of a larger one the model is told what it is - name, type, size, about how many
  tokens - and reads parts of it through a tool (`read_attachment`, with a range), as `read_file`
  reads a workspace's files. The tool reads only the renditions of attachments in the turn's
  context, and asks for no permission of its own. Open: the threshold - configured, or a share of
  the model's context window (`ModelProfile`) - and whether images are kept to it as well.
- **Other participants.** Every agent of the channel hears the message with its attachments and
  gives its model what the model can take. An external agent (A2A, file parts) is told of the
  attachment in text at first; handing on the content is a question of its own.
- **Tool results.** Images in a tool result are possible with Ollama, the Responses API, llama.cpp
  and vLLM, but not with OpenAI's Chat Completions. The type should be one that a `ToolCallResult`
  can carry later (208, MCP image content); a client for an API without it puts the image into a
  message of its own after the result.

**Tried with Ollama 0.40.6 on 2026-10-24**: an image showing "KIWI 7319" in black on orange, given
once in a user message, once in a `tool` message's `images` after a call of `view_attachment`, and
once as the same tool message without the image.

| Model | In a user message | In a tool result | Tool result without the image |
|---|---|---|---|
| gemma4:e4b | read | read | made up: "NO PARKING ANY TIME" |
| gemma4:26b | read | read | made up: "Hello World" |
| qwen3.6:27b | read (and asked for it by the tool besides) | read | made up: "Hello World" |
| gemma4:cloud | read | read with `think: true`, twice; with `think: false` once garbled before the answer, once no answer within 600 s | made up: "Hello World" |

So an image can be given as a tool's result. And every model makes up an image it was told of
but not given - where an attachment cannot be shown, the text has to say so, and a tool never
reports success without the image.

### What a model is given in the first step

**Decided:**

- **Images are given along to a model that can see**, in the message they are attached to.
  `view_attachment` gives one as a tool's result - one that is not given along: older, above the
  threshold, left out by a compaction.
- **Text** - plain text, markdown, CSV - is read through `read_attachment`, with a range, straight
  from the storage, nothing converted.
- **Everything else is worked on in a workspace.** A tool copies an attachment into the workspace
  the turn may use - "attachment `a3` to `docs/report.pdf`", by default under `attachments/` -,
  and the model works on it there with what it has (`read_file`, the terminal). The mirror image of
  `publish_artifact`: the workspace is where a copy is worked on, never where an attachment lies.
  Copied, it is a file of the workspace like any other - its lifetime, its label, shared as the
  workspace is -, and copying is a flow of the message's classification into the workspace's
  (`may_publish`), checked as writing what the channel showed into the workspace is. In a channel
  without a workspace such a document cannot be worked on until central conversion comes, and the
  model is told so in the note. Whether an agent needs a workspace of its own beside the shared one
  was asked and is not pursued: it would concern everything an agent writes, not attachments.
- **The tools are offered where the agent's transcript holds attachments** (`offered`, issue 290):
  offering is a convenience, the call checks for itself. Today `offered` is given the turn's
  `Situation` before the context is built; whether there are attachments is an input it does not
  have yet.
- **Every message with attachments carries a note** in the context, in the format of its sender's
  marking (`sender_format`), one per attachment: its number, name, type, size and whether it is
  given along - with `xml` such as `<attachment ref="a3" name="report.pdf" type="application/pdf"
  size="2.3 MB" shown="no"/>`.

**Decided: a model names an attachment by a number of its transcript.** A UUID costs tokens and is
copied wrong. Each agent numbers the attachments as they enter its transcript - `a1`, `a2`, ... -,
so that the same image may be `a3` to one agent of a channel and `a1` to another; each model sees
its own numbers only. The agent's attachments in its channel are thus a view, translating between
the `AttachmentId` and its number - no storage of their own:

- **id to number** when the context is built, for the note; **number to id** when a tool is
  called. A number that exists is what the agent heard: a tool reaches the attachments numbered in
  the agent's transcript that still stand, nothing else - no message of another channel, none it
  did not hear, no older version. Reading one joins the turn's label with its message's, as any
  reading does. Since it is the transcript and not the context, an attachment of messages a
  compaction summarised stays reachable.
- **The number is recorded in the transcript's entry** the attachment entered with - what the model
  saw, covered by the digest chain, without changing the message all agents share - and **the
  translation is kept in the agent's state** (`AgentChannelState`) with the next number, as its
  sums are: an agent holds only a window of its chain, and a compaction starts a new branch, while
  a number has to resolve beyond both. The index can be rebuilt from the chain.
- **A number is never given again.** An attachment a modification keeps keeps its number - the
  translation goes by `AttachmentId` -, one added gets the next, one removed is no longer found.
- **An external agent's** transcript starts anew with every call, and so do its numbers.
- **Not the owner.** The transcript owns how its model names an attachment and what the model was
  given of it; the content, its lifetime and who reads it stay with the message and its channel.
  An attachment lies in many transcripts, users have none, and erasing needs one place to find it.

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
- An attachment's content lies in a binary storage of the core, which artifacts use as well; an
  attachment refers to it by storage and key, and 208 knows which type it uses for what a model is
  to see.
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
- **2026-10-24** — Decided with the maintainer (*How attachments are managed*, *Converting*, *The
  context window*): attachments are treated alike whatever their source - a tool's artifact, an
  image a model generated, a file uploaded with the message or before it. An attachment is part of
  its message and names its content by storage and an opaque key, the digest beside it as what the
  content is checked against; a storage holds keys and bytes, without permissions. The binary
  storages move from the artifacts plugin into the core, as a step of their own. Content from
  another source, an artifact's version among them, is attached by copying it within the storage;
  what a user uploads directly is not versioned. The original is attached; a rendition belongs to
  its attachment, is made once and removed with it; the transcript records only which rendition a
  model was given. A large document is read through a tool, not put into the context whole.
- **2026-10-24** — Decided with the maintainer (*Classification, changes, taking back*): an upload
  not yet sent has no label - only the channel's classification is checked against the
  `destination` of the storage attachments are kept in; a sent attachment has its message's
  classification and none of its own, and raising a message is checked against its attachments'
  storage; attaching an artifact's version whose classification may not flow into the message's is
  refused, not answered by raising the message's label.
- **2026-10-24** — Decided with the maintainer (*Classification, changes, taking back*): a
  modification may remove and add attachments; `MessageRequest::attachments` names them as they are
  to stand, for a new message and a modification alike. An older version keeps its attachments as a
  record, their keys until the message is erased or its session purged; only what stands is read
  and downloaded. An attachment has an id of its own within its message (`AttachmentId`), by which
  it is downloaded and its key is made. Taking a message back empties its attachments; a tool
  result's attachments stay with their result.
- **2026-10-24** — Decided with the maintainer (*How attachments are managed*, *Converting*, *What
  a model is given in the first step*): an attachment lies in its channel's scope and goes with its
  purge; its key is `<channel>/<attachment id>`, fixed at the upload - not "the session and the id"
  as written earlier today -, and "not yet sent" is a record, not a place. Central conversion is
  deferred: images are given along to a model that can see and through `view_attachment`, text is
  read through `read_attachment`, everything else is copied into a workspace and worked on there.
  Tried with Ollama first: images in a tool's result are seen; an image a model is told of but not
  given is made up. A model names an attachment by a number of its agent's transcript; the agent's
  attachments are a view translating between `AttachmentId` and that number, the number recorded
  in the transcript's entry, the translation in the agent's state. A workspace of each agent's own
  was asked about and is not pursued.
