# 282 — A user cannot attach an image or a file to a message

**Status:** in progress
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

**Revised: each attachment has a classification of its own.** The decision above that a sent
attachment has none argued from its readers alone - who reads the message reads it. But a label
also decides the flows, and there it matters: a turn's label grows only with what its model is
actually given, so an agent that reads an `internal` message without looking at the `confidential`
contract attached to it stays `internal` and may go on writing into an `internal` workspace;
copying an attachment into a workspace, keeping it as an artifact or reading it from another channel
asks for that attachment's label, not the message's. A `ToolCallResult` has a classification of
its own within its message for the same reason.

- **When a message is sent**, each attachment is given a label - the message's unless the user
  says otherwise -, and each is checked on its own: that it may flow into the channel, and into the
  `destination` of the storage. Before it is sent an upload still has none.
- **An attachment is reclassified on its own**, by the rule a message is; a channel lowered fits
  each attachment as it fits each message.
- **An artifact's version attached** takes the artifact's classification, which only has to flow
  into the channel; the rule above that refuses one whose label may not flow into the message's
  is void.
- **The note** at the message - number, name, type, size - is the message's and carries its label;
  a turn's label joins an attachment's when its model is given the content.

### What is said and what is seen

**Decided: an attachment is what was said; what a tool gave its model is seen, not attached.** To
the channel an agent is to feel like a person: a message has a list of attachments, each with its
own label, whether a user or an agent sends it, and the channel enters them by one rule. The line
drawn is not between users and agents, but between what is said and what is seen. A person who
looks at an image on their screen has said nothing; a tool's result is that for an agent - what its
model was given, which the other agents do not hear (`Message::as_heard` drops tool results). A
person who wants to show it attaches it to what they say; an agent does the same, deciding which
image it shows the others and when, at the price of a step of its own.

- **What a tool gave is kept as seen:** `ToolCallResult::media`, of the same type of reference as
  `Message::attachments` - storage, key, id, name, type, size, digest, label -, but no attachment.
  It has to be stored all the same, since every model call gives it again and the transcript
  refers to it. It follows its result: its label (the result's unless the tool says otherwise) and
  its visibility - a result withheld from the channel withholds its media (`published`); the card
  of the call shows an image as it shows the result's text.
- **A tool hands over bytes, the `ToolExecutor` stores them.** `ToolOutput` gains, beside its value
  and classification, the media as bytes with a name and a media type; the executor, which already
  makes the `ToolCallResult` of the output, puts them into the storage and sets the references.
  Limits, sniffing the media type and the allow-list are applied there, once for every tool; no
  tool reaches the storage, as no client does.
- **`attach` shows something.** A tool of the same component as `view_attachment` and
  `read_attachment`: `attach("a5")` resolves the number through the transcript, copies the content
  within the storage under a new `AttachmentId`, and puts it aside in the turn, with the label of
  where it came from - as `create_plan` keeps its plan in the turn (`ToolProvider::execute` is given
  `turn: &mut Turn`). The next message of the turn that others hear - as a rule its answer -
  carries what was put aside, text and image arriving together as a person's do. What has a number
  can be attached: an image a tool gave, the main case, and an attachment heard from someone else,
  to pass it on. A turn that ends without such a message - `silence`, given up - claims nothing,
  and what it put aside expires. Not `publish_…`: those offer a workspace file or an artifact in a
  card, which is something else.
- **An artifact is not the way to show something.** An artifact is kept - named uniquely in its
  scope, versioned, managed among its owner's -, not said; published, it is a tool's result,
  which no other agent hears; it is copied from a workspace, where a tool's image does not lie; and
  it is a plugin's, without which no agent could show anything. Showing and keeping stay two acts;
  `publish_artifact` may put the version aside as an attachment of the answer besides, through the
  same method of the turn - whether by default is the plugin's to decide.

**Decided: whoever puts the content gives the id; the channel enters it.**

- **`put`** stores bytes under `<channel>/<attachment id>` and gives the id - a UUID, which needs
  nobody to hand it out. The owner of attachments records it as *not yet claimed*: by whom, when,
  in which channel. The content has to lie there before the message passes the channel, since bytes
  do not go through the transport; so the id exists from then on, and an agent has it from the
  moment its tool's content is stored. Who puts: the REST handler for a user's upload, through the
  views - as a workspace upload streams to the terminal the views hand it, the runtime asking
  whether the user may write in the channel -, not through the user's actor, which queues what its
  user says and would carry megabytes through its transport and start a channel that does not run;
  the `ToolExecutor` for a tool's media; `attach` by copying; the actor of an external agent for
  its file parts (208).
- **The channel enters it when it admits a message**, where every message passes and is already
  checked: each new attachment of the message, and the media of its tool results, put by its sender
  in this channel and not yet claimed - claimed exactly once, so that no two messages ever share a
  key. One place rather than the three producers each. A message being modified may name the
  attachments it has besides.
- **What nothing claims expires**: an upload never sent, what a turn put aside without saying it,
  and a tool's media whose turn broke off before its result was recorded - no key is left behind
  that nothing refers to. The records of what is not yet claimed lie with the owner of attachments,
  not in the channel's state, since an upload must not start the channel.
- **No entity of its own.** What is known of an attachment stands in its message; a document of its
  own in the `EntityStore` would hold it twice. Only what is not yet claimed needs a record.

**Decided (proposed first, confirmed by the maintainer the same day):**

- **`view_attachment` shows, it brings nothing.** Its result names the attachments it shows
  (`ToolCallResult::shown`, their ids) instead of storing their content again under a new key; the
  turn puts the content on the copy the model is given, as for a message's attachments. If the
  message it came from is erased, the transcript keeps the digest, and the result reads as no
  longer available.
- **The cut with [208](208-generated-artifacts.md).** 282 brings the type, `ToolCallResult::media`
  and `shown`, the way through `ToolOutput` and the `ToolExecutor`, and `attach`. 208 keeps taking
  in what comes from outside: an MCP result's `image` and `resource` contents as media - which also
  ends `mcp_tool` putting a result without text, base64 and all, as JSON into the context
  (`call_tool_result_to_value`) -, an A2A answer's file parts as attachments of the external
  agent's message, and the question of files given by URL. Its panel of a channel's artifacts would
  become a list of a channel's attachments, if it is still wanted.

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

## Plan

Proposed 2026-10-24 from the decisions above, checked against the code as it stands. Six steps,
each a change of its own, reviewed and committed on its own; the first is a refactoring without a
change of behaviour. What the checking found that the decisions above do not say yet is under
*Checked against the code*; what has to be decided before the steps are built is under *Open*.

### Checked against the code

- **A download is never shown inline.** REST-SERVER.md: every download is
  `application/octet-stream`, `Content-Disposition: attachment`, `nosniff`, `no-store`, so that
  HTML or SVG never runs in the web UI's origin (`attachment_response`). *Upload and the REST API*
  above says images of the allow-list are shown inline. Open 1.
- **A message without text is heard by nobody.** `Message::is_heard` requires non-empty
  `content`; `Recipients::members(message.is_heard())` sends it to the agents only then, and
  `Message::published` gives nothing of a message whose content, reasoning, calls and results are
  all empty. A message of an image alone would reach no agent. Both take attachments into account
  (step 2).
- **The digest.** `digest::message_chain` covers `tool_results` as JSON, so `media` and `shown`
  are covered as they are added; `attachments` are added to the scheme as a version of its own,
  `caelumora.message-chain.v2`. `message_modified` covers content, reasoning and classification
  alone and takes the attachments over as well. Stored histories and transcripts have to be
  deleted.
- **The numbers cannot be found over the chain.** A note's next number is the highest on any
  branch plus one (`Ledger::next_id`); but an agent holds only a window of its transcript
  (`keep_window`), and the store alone has the rest. The translation and the next number are a part
  of `AgentChannelState`, as `last_message` and `used` are (step 4).
- **Not only the turn calls the model.** Compaction asks the agent's model itself, with
  `Turn::context_until` (`summary_compaction`), and an external agent gives its remote agent the
  transcript through `ModelProfile::given_messages`. Loading content goes where the messages for a
  model are built from the transcript - `Turn::full_context` and `context_until` -, not into
  `handle_planning`; the external agent is told of an attachment in text alone (step 4).
- **The profile is fixed as the agent is built.** `ModelProfile` comes from the
  `ModelProfileProvider`, read once as the components are built; `/api/show` is read only for the
  control tokens, at the first call. Whether a model sees images cannot be learnt lazily. Open 3.
- **`ContentDigest` hashes the length first**, so an upload is digested as it streams only with
  its `Content-Length`; one without is refused (`411`).
- **Nothing sniffs media types, nothing caches by a byte budget**, and no crate for either is a
  dependency. Both are written in the core: the magic bytes of png, jpeg and webp (`media::sniff`,
  a dozen lines), and a cache of `Bytes` by key within a budget (`ContentCache`). The Ollama client
  needs `base64` (added to the workspace dependencies) to send images.
- **`validate_users_own` forbids a channel's scope** for workspaces and artifacts. An attachment
  is no entity and lies in no store, so the rule does not touch it; its key names the channel.
- **The key names the session too.** A session is purged by one `purge_lying_in(SessionScope)`
  after its channels' documents are gone, so a member cannot list the channels then. The key is
  `attachments/<session>/<channel>/<id>`: a session purges `attachments/<session>/`, a channel
  `attachments/<session>/<channel>/` - the same removal by prefix. Artifacts' keys become
  `artifacts/<id>/<n>` beside them in the same storage.
- **`attach` is a word taken**: `attach_workspace`, `PUT /sessions/{id}/workspace` attach a
  workspace to a session. The tool keeps the name - it attaches to a message, which the model
  reads as plain English -, the code says `Turn::put_aside` and `attachments`, never `attach`.
- **`view_attachment` needs more than ids.** To be given to the model again, a shown attachment
  needs its storage, key and media type; resolving ids through the transcript at every call is
  work for nothing. `shown` holds the references themselves - copies of the `Attachment`, the same
  id and key, no new content - which keeps what the model saw covered by the digest.

### Step 1: the content storages in the core

A refactoring: the storages of artifacts become storages of content, of the core, in the shape
they have (`ArtifactStorage` → `ContentStorage`), keyed by opaque strings. Pattern: the workspace
storages - a trait with a `local` backend registered in `Backends::built_in`, made by
`RuntimeBuilder::new` from a section of `AppConfig`, held by whoever uses them.

`caelumora-runtime/src/content/mod.rs`, `local.rs`:

```rust
/// Where binary content lies (issue 282): keys and bytes, nothing of permissions, which are
/// the owner's of what the key names - an artifact's version, a message's attachment -, each in
/// a key space of its own. The service a storage of content is a backend of (issue 324): the
/// core brings `local`, a plugin may register another (S3, issue 272).
#[async_trait]
pub trait ContentStorage: Send + Sync + fmt::Debug {
    /// Keeps `content` under `key`, at most `limit` bytes, or nothing; answers the bytes kept.
    async fn put(&self, key: &str, content: ByteStream, limit: u64) -> ContentResult<u64>;
    /// The content under `key`, with its size.
    async fn get(&self, key: &str) -> ContentResult<(u64, ByteStream)>;
    /// Removes what lies under `key`; nothing there is no error.
    async fn delete(&self, key: &str) -> ContentResult<()>;
    /// Removes everything whose key lies under `prefix` - whose leading segments it is; nothing
    /// there is no error.
    async fn remove_under(&self, prefix: &str) -> ContentResult<()>;
    /// What may be put into it: the classification content in it may have at most; `None` for
    /// no bound.
    fn destination(&self) -> Option<&Classification>;
}

#[derive(Debug, thiserror::Error)]
pub enum ContentError { InvalidKey(String), NotFound(String), TooLarge { limit: u64 }, Unavailable(String) }
pub type ContentResult<T> = Result<T, ContentError>;

/// The backends a section of the configuration names, by their names (`caelumora_runtime::backend`,
/// what `Backends::make_named` answers - the storages of workspaces too).
pub struct NamedBackends<T: ?Sized>(BTreeMap<String, Arc<T>>);
impl<T: ?Sized> NamedBackends<T> {
    /// The backend `name`, if one is configured.
    pub fn get(&self, name: &str) -> Option<&Arc<T>>;
    /// The name of the one to use: `named`, or the one there is; `None` where none is.
    pub fn chosen(&self, named: Option<&str>) -> Result<Option<&str>, Unchosen>;
}
```

- A key is a path of segments, none empty, `.` or `..`, checked by `local` before it touches the
  file system; `<root>/<key>`, written through `write_whole`.
- `caelumora-model/src/config.rs`: `ContentConfig { storage: BTreeMap<String, BackendConfig> }`,
  section `content`; `RuntimeBuilder::new` makes them (`"storage of content"`) and offers them,
  `pub fn content_storages(&self) -> &Arc<NamedBackends<dyn ContentStorage>>`.
  `RuntimeBuilder::make_named`, whose one user the plugin was, goes.
- `caelumora-artifacts`: `ArtifactsConfig` loses `storage` and keeps `max_bytes` and `publish_to`,
  now naming a storage of content (`NamedBackends::chosen`, the rule the config held before); the
  manager takes the storages from the builder and keys `artifacts/<id>/<n>`; `local.rs` goes;
  `remove` becomes `remove_under`.
- Docs: CONFIGURATION.md (section `content`, `artifacts` row), STORAGE.md (*Artifact content* →
  *Content*), ARCHITECTURE.md (backends), `examples/config.yaml`; the tests of `local` move to the
  core, those of the plugin configure `content`.
- Stored data to delete: the content of artifacts (`local/artifacts`), which lies under other keys
  now.

### Step 2: the attachment in the model

`caelumora-model/src/attachment.rs`:

```rust
define_id!(AttachmentId);

/// What a message carries besides its text (issue 282): what is known of the content - its name,
/// type, size and digest -, how it is classified, and where its bytes lie. Part of the message
/// it is attached to: classified with it, taken back with it, covered by its digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: AttachmentId,
    /// A file name, as uploaded, at most 255 characters, without a path.
    pub name: String,
    /// Sniffed from the bytes, never taken from the client (`media::sniff`).
    pub media_type: String,
    pub size: u64,
    /// `digest::content` of the bytes: what they are checked against, never a key.
    pub digest: Digest,
    /// Its own (issue 282): the message's unless its sender said otherwise; never above the
    /// channel.
    pub classification: Classification,
    /// The storage of content its bytes lie in, as configured, and its key there.
    pub storage: String,
    pub key: String,
    /// The number the agent whose transcript holds this copy knows it by (`a3`); `None` in the
    /// channel's record and in every other copy. Given by `AgentChannelState` alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// The bytes, on the copies a model is given and nowhere else: never stored, not covered by
    /// the digest; a clone counts a reference.
    #[serde(skip)]
    pub content: Option<Bytes>,
}

impl Attachment {
    /// Whether a model of `profile` is given the bytes with the message, or told of them alone.
    pub fn given_along(&self, profile: &ModelProfile) -> bool;
    /// `a3`: how the model names it; `None` while it has no number.
    pub fn reference(&self) -> Option<String>;
}

/// The media types an attachment may have (issue 282): png, jpeg and webp, told from the first
/// bytes, and plain text; `None` for anything else.
pub mod media { pub fn sniff(head: &[u8]) -> Option<&'static str>; pub fn is_image(media_type: &str) -> bool; }
```

- `Message::attachments: Vec<Attachment>`, absent where empty, as `tool_calls` is;
  `ToolCallResult::media: Vec<Attachment>` - what the tool gave its model, seen - and
  `shown: Vec<Attachment>` - what `view_attachment` showed again, references of attachments heard.
- `Message::new`, `ToolCallResult::of` list the fields. `is_heard`: text or attachments.
  `as_heard` keeps them. `published`: a result withheld from the channel loses `media` and `shown`
  as it loses its content; a message with attachments alone is published. `names_only` drops both.
  `modify` takes the attachments over; `revoke` clears them. `estimated_tokens` counts an image as
  768 tokens (Ollama's estimate; 465 refines it).
- `digest::message_chain` v2 adds `attachments` (id, name, media type, size, digest, number);
  `message_modified` v2 adds them with their classifications.
- `HistoryField::Attachments` for a filter, so that a channel's attachments can be listed later
  (208) - only if cheap; else left out.
- Tests: the rules of `is_heard`, `published`, `modify`, `revoke` with attachments; the digests;
  `media::sniff` on the magic bytes.
- Stored data to delete: every history and transcript (the digest scheme changed).

### Step 3: the owner of attachments and what enters the channel

`caelumora-runtime/src/attachment/{mod.rs, manager.rs, cache.rs}`. Pattern: a module's manager -
the one owner of a lifecycle -, held by the `RuntimeManager`, a `ScopeMember`, offered as a
service, reached by an actor through its `ActorRuntime`.

```rust
/// The one owner of the content of attachments (issue 282): it keeps what is put - by a user's
/// upload, a tool's media, a copy to attach -, records it as not yet claimed, hands it to the
/// message that claims it as the channel admits it, lets what nothing claims expire, reads
/// content for a model or a download through a cache per process, and removes a channel's or a
/// session's attachments with it.
pub struct AttachmentManager { /* storages, keep_in, config, pending: Mutex<HashMap<AttachmentId, Pending>>, cache: ContentCache, sweeper */ }

impl AttachmentManager {
    pub(crate) fn new(storages: Arc<NamedBackends<dyn ContentStorage>>, confidentiality: Arc<dyn ConfidentialityPolicy>, config: &AttachmentsConfig) -> ComponentResult<Arc<Self>>;
    /// Keeps `content` as an attachment put by `by` into `channel`, not yet claimed: sniffs the
    /// type, refuses one not allowed, bounds the size, digests as it streams; the channel's
    /// classification has to flow into the storage's `destination` (`require_storable`). In a
    /// user's name: the caller takes part in the channel. Answers the reference, without a
    /// classification yet (`None`).
    pub async fn put(&self, ctx: &SecurityContext, channel: &Channel, by: ActorId, name: &str, size: u64, content: ByteStream) -> AttachmentResult<Attachment>;
    /// Keeps a copy of `source`'s content as a new attachment put by `by`, not yet claimed,
    /// classified as the source: `attach` of an agent, a version of an artifact (the plugin).
    pub async fn copy(&self, by: ActorId, channel: &Channel, source: &Attachment) -> AttachmentResult<Attachment>;
    /// Whether every one of `ids` is not yet claimed, put by `by` into `channel` - what the
    /// channel asks before it admits a message.
    pub fn claimable(&self, by: ActorId, channel: ChannelId, ids: impl Iterator<Item = AttachmentId>) -> bool;
    /// Claims them for the message that entered: taken out of what is pending, exactly once.
    pub fn claim(&self, ids: impl Iterator<Item = AttachmentId>);
    /// The bytes of `attachment`, from the cache or the storage, checked against its digest.
    pub async fn content(&self, attachment: &Attachment) -> AttachmentResult<Bytes>;
    /// The bytes as a stream, for a download; the cache is not filled.
    pub async fn stream(&self, attachment: &Attachment) -> AttachmentResult<(u64, ByteStream)>;
    /// Puts the bytes on every attachment and medium of `messages` that `given_along` says,
    /// loading what the cache does not hold; one that cannot be had stays without, logged.
    pub async fn load(&self, messages: &mut [Message], profile: &ModelProfile) -> AttachmentResult<()>;
}
impl ScopeMember for AttachmentManager { /* delete and restore do nothing; purge removes the prefix of the scope */ }
```

- `AttachmentsConfig`, section `attachments`: `max_bytes` (50 MiB), `max_per_message` (10),
  `keep_in` (the storage, chosen as `artifacts.publish_to` is: `NamedBackends::chosen`),
  `cache_bytes` (256 MiB), `unclaimed_expire_secs` (3600). Without a storage, an upload fails saying
  so (`NotConfigured`).
- **`ContentStorage::copy(from, to)`** comes with its first caller, here: keeps a copy of what
  lies under `from` under `to`, as cheaply as the backend can - a hard link or reflink,
  `CopyObject` -, two keys each removed on its own; `local` tries a hard link and falls back to
  copying the file.
- **Pending** is a record in memory: who put it, into which channel, when. A sweeper, built as
  `CommandReaper` is (an interval task aborted on drop), deletes what expired. What a crash
  leaves unclaimed is removed with its channel (`remove_under`), not before - accepted. Open 2.
- **`ContentCache`**: `Bytes` by key within `cache_bytes`, least recently used first; `forget(key)`
  for erasing (237) and purging.
- **The channel** (`ChannelLoop::admission`): for every message of the event, each attachment's
  classification and each result's media's flows into the channel (as the parts do today); the
  new attachments of the message - all of a new one, those the original lacks of a modification -
  and the media of its results are `claimable` by the sender, else `Rejection::Invalid`; every
  `shown` lies in this channel (its key's prefix). In `enter`, after `record`, `claim`. A
  modification keeps the attachments of the original only by their ids: `Message::modify` takes
  the list of the new version over as it stands.
- **`UserActor::send_message(content: Option<String>, classification, attachments: Vec<Claimed>)`**
  where `Claimed { id: AttachmentId, classification: Option<Classification> }` names an upload
  and, if the user says so, its label; the actor reads the reference of each from the manager
  (`pending(id)`), gives it the message's label where none is named and puts it on the message;
  `modify_message` the same, the ids of the message's own attachments kept as they stand. A
  message needs text or an attachment (`ChannelError::Invalid` otherwise).
- **REST** (`caelumora-rest-server/src/routes/attachments.rs`):
  `POST …/channels/{cid}/attachments?name=` - the raw body with `Content-Length` (`411`
  without), through `channel_of` (reach) and the
  manager's `put` -, answering the `Attachment`;
  `GET …/messages/{mid}/attachments/{aid}` through
  `channel_of` and `ChannelView::message` (the reader's view, the published form), served by
  `attachment_response` - or inline, open 1. `MessageRequest` gains `attachments:
  Vec<AttachmentRequest>`, each `{ id, classification: Option<Classification> }`, and its
  `content` becomes optional. The TUI sends no
  attachments (`Vec::new()`).
- **Runtime**: `RuntimeBuilder::new` makes the manager with the content storages, offers it
  (`services`, `scope_members`); `RuntimeManager::attachment_manager()`;
  `ActorRuntime::attachments() -> &Arc<AttachmentManager>`.
- Tests: the manager (put, sniffing, limit, claim once, expiry, purge by prefix, the cache's
  budget); the channel's admission (a message naming an upload of someone else, one claimed
  before, one of another channel, a label that may not flow); the security suite: uploading asks
  the reach of the channel and the flow into the storage, downloading asks what reading the
  message asks, a stranger gets `404`; the modification cases.
- Docs: CHANNELS.md (*What enters a channel*), REST-SERVER.md (endpoints, the download rule),
  STORAGE.md (*Content*: attachments beside artifacts), SECURITY.md and PERMISSION-CHECKS.md
  (upload, download, admission rows), CONFIGURATION.md (`attachments`), `examples/config.yaml`.

### Step 4: numbers, the note, and the images a model is given

- **`AgentChannelState::attachments: AttachmentIndex`** - `numbers: BTreeMap<AttachmentId, u32>`,
  `next: u32` -, with `AgentChannelStateChange::Attachments`; `number(&mut self, id) -> u32`
  gives the next number once and the same again. `perceive` and `said` number every attachment,
  medium and shown of what enters the transcript, on the transcript's copy
  (`Attachment::number`); a modification taken in numbers what is new. `Situation::attachments()`
  answers the index, by which a tool resolves `a3` and `offered` sees whether there are any.
- **`ModelProfile::images: bool`** (`#[serde(default)]`): whether the model is given images. Set
  by `ollama_client` from its parameter `vision` - open 3 -, `false` for `a2a_agent`.
- **The note**: `ContextResolver::given_path` appends to the content of a message with
  attachments, before the sender is marked, the text `core.attachments` rendered with the
  profile's `sender_format` and the attachments (`reference`, `name`, `media_type`, `size`,
  `given_along`): with `xml` one `<attachment ref="a3" name=".." type=".." size=".." shown="no"/>`
  a line, else `[attachment a3: report.pdf, application/pdf, 2.3 MB, not shown]`. The agent's own
  messages are marked alike, so that it knows what it attached. A medium of a tool result is
  described by the tool's own content, not by a note.
- **Loading**: `Turn::full_context` and `context_until` become `async` and, after resolving,
  call `attachments.load(&mut messages, &profile)`; the messages hold the `Bytes` for the call.
  An external agent loads nothing.
- **`ollama_client`**: `OllamaChatMessage::images: Option<Vec<String>>`, base64 of every
  attachment of a `user` message and every medium and shown of a `tool` message that carries
  content; `vision: bool` in `OllamaClientConfig`; LLM-MODELS.md says which models see.
- Tests: the index gives a number once; the note in both formats; `full_context` puts content
  where the profile says and nowhere else; the client sends `images`.
- Docs: AGENT-TURN.md (*Who said what*, *The turn's context and the resolver*, what is held),
  CONFIGURATION.md (`vision`), COMPONENTS.md (`ollama_client` row), LLM-MODELS.md.

### Step 5: the tools

`caelumora-agents/src/tool/attachment_tool.rs`, kind `attachment_tool`, `ToolProvider`,
`uses::<AttachmentManager>()`, texts under `agents.tools.attachment_tool`; registered in
`tool::register_all`. Offered where `Situation::attachments()` is not empty.

| Tool | Parameters | Does | Approval, visibility, classification |
|---|---|---|---|
| `view_attachment` | `ref` | Gives the model an image again: the attachment as `shown` of its result, loaded with the message. Refuses one that is no image, with its type. | none; `SHOWN`; the attachment's label |
| `read_attachment` | `ref`, `start_line?`, `line_count?` | Text of an attachment of a text type, bounded as `read_file` bounds it, saying where to read on. | none; `SHOWN`; the attachment's label |
| `copy_attachment` | `ref`, `path?` (default `attachments/<name>`) | Writes the attachment's bytes into the workspace the turn may use (`Turn::workspace_terminal(Grant::Write)`), where its label may flow into the workspace's (`may_publish`); answers the path. | where changes are (`requires_approval`, as `terminal_tool`); `SHOWN`; the workspace's |
| `attach` | `ref` | Puts a copy aside for the turn's next message that others hear (`Turn::put_aside`); answers that it will be attached. | none; `SHOWN`; the attachment's label |

- `ref` is resolved through `Situation::attachments()`; an unknown one is the model's to correct.
  Reading one records knowledge (`KnowledgeSource::Attachment { id, digest }`) and the turn sees
  its label (`Turn::see`).
- **`TurnState::attachments: Vec<Attachment>`** - put aside, pending -, `Turn::put_aside`,
  `Turn::take_aside`; `AgentActor::stamp` of a complete message that is heard (`!internal`) moves
  them onto it. A turn given up leaves them pending; they expire.
- **`ToolOutput::media: Vec<Media>`**, each a `name`, a `media_type` if the tool says, `bytes`;
  `ToolExecutor::admitted` puts each through `AttachmentManager::put` as the agent, classified as
  the result, into `ToolCallResult::media`; a put that fails fails the result. No built-in tool
  gives media yet; 208 brings the first (MCP).
- Tests: each tool (the number resolved, the refusals, what is recorded), `attach` to the answer
  and nothing to a plan step, the executor keeping media, the agents' security suite
  (`copy_attachment` asks the workspace's questions, nothing else asks).
- Docs: TOOLS.md (*Attachments*), COMPONENTS.md (the row), AGENT-TURN.md (*What leaves the
  agent*), APPROVALS.md (*What needs approval*).

### Step 6: the web UI

`caelumora-web-ui`: a paper clip at the message field, drag and drop onto the chat, paste from the
clipboard (`onPaste` with files); each file uploaded at once (`api.uploadAttachment`), shown
before sending with its name, size, an image as a preview and a remove; sent with the message
(`attachments` in the request), a message of attachments alone allowed; at a message, images as
previews and everything else with name, type, size and a download; the tool card of a call with
`media` or `shown` shows them alike; `MessageModified` takes the attachments over; the types in
`src/types` mirror the API. Docs: WEB-UI.md (*Input*, *A channel*).

### Later, not in these steps

- Reclassifying one attachment after it was sent (*Revised* above): an act of its own
  (`AttachmentReclassified`), through every chain and the REST API; until then an attachment's
  label is set as the message is sent or modified, and fitted with the channel.
- A bound on what is given along (465); erasing (237); media of MCP results, A2A file parts, a
  list of a channel's attachments (208); central conversion (*Converting* above); S3 (272).

### Open

1. **Inline images.** The rule "a download is never shown inline" exists so that HTML or SVG
   never run in the UI's origin; png, jpeg and webp run nothing. Either the download of an
   attachment of the allow-list is served with its media type and `inline` (still `nosniff`,
   `no-store`), and REST-SERVER.md names the exception - the UI shows it with a plain `<img>` -;
   or the rule stays and the UI fetches the bytes and shows a blob URL. Recommended: the
   exception, named.
2. **What is pending lives in memory.** Lost with a restart: an upload made before it has to be
   made again (the UI answers a `404` at sending so), and its bytes stay until the channel is
   purged. The alternative is to keep the records in the `UserChannelState` - a user in one
   channel - which the `UserController` writes alone, while claiming is the channel's act: two
   writers, or a round trip. Recommended: memory.
3. **Whether a model sees.** `vision` of `ollama_client`: `true`, `false`, or `auto` - asked of
   `/api/show`'s `capabilities` as the component is built (an async factory); a server that
   cannot be asked gives `false` and a warning, since the first call fails anyway while it is
   down. Today nothing of a model is read as it is built. Recommended: `auto` by default, so that
   the example configuration needs nothing.
4. **The number on the transcript's copy** of an attachment (`Attachment::number`, `None`
   everywhere else) rather than a field of the transcript's entry: the copy a transcript holds is
   already the agent's own (`as_heard`), and the digest covers the number so. The alternative
   changes `TranscriptEntry::Message` and everything that reads it.
5. **A message without text**, an image alone: allowed, heard, published and shown. The
   moderator's policies and the decision model are given messages with text today; one without
   is given its note (`core.attachments`) as its text where a model reads it. Decided so by the
   maintainer on 2026-10-24.

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
- **2026-10-24** — Decided with the maintainer (*Classification, changes, taking back*, *What is
  said and what is seen*): each attachment has a classification of its own after all - a label
  decides flows, not only readers -, the message's unless the user says otherwise; an artifact's
  version attached takes the artifact's. What a tool gives its model is seen, not attached
  (`ToolCallResult::media`, the result's label and visibility, shown in the call's card as its
  text is); an agent shows something by `attach`, which puts a copy aside in the turn for its next
  message heard - not by publishing an artifact, which keeps rather than says. Whoever puts the
  content gives its id; the channel enters a message's new attachments and its results' media when
  it admits the message, claiming each once; what nothing claims expires. A user's upload goes
  through the views, not through the user's actor. Proposed, not yet confirmed: `shown` for
  `view_attachment`, and the cut with 208.
- **2026-10-24** — Confirmed by the maintainer: `shown` for `view_attachment`, and the cut with 208
  (*What is said and what is seen*); 208's history says so.
- **2026-10-24** — The last question open under *The context window* - where the line lies between
  what is given along and what is fetched through a tool, configured or a share of the model's
  context window, and whether images are kept to it as well - moved to its own issue,
  [465](465-attachments-in-the-context.md), at the maintainer's request.
- **2026-10-24** - Step 1, the content storages in the core, built: `ContentStorage` with `local`
  in `caelumora-runtime/src/content/`, the section `content`, `ContentStorages` (`get`, and
  `chosen` - the rule `ArtifactsConfig::publish_to` held, to be shared with attachments), the
  artifacts keeping their versions under `artifacts/<id>/<n>`. Built otherwise than planned, and
  the plan says so now: `copy` comes with step 3, its first caller; the prefix is removed by
  whole segments (`remove_under`); no type of its own for a key; the builder hands out
  `&Arc<ContentStorages>`; `RuntimeBuilder::make_named` is gone with its one user, so that a
  plugin no longer has backends of a service of its own. Stored data to delete: the content of
  artifacts, which lies under other keys now; the section `artifacts.storage` of a configuration
  becomes `content.storage`.
- **2026-10-24** - Before step 2, decided with the maintainer and built as a refactoring of its own:
  the backends a section names are one type for every service, `NamedBackends<T>` of
  `caelumora_runtime::backend` (`get`, `chosen`, `iter`), which `Backends::make_named` answers - the
  storages of workspaces (`WorkspaceManager`, `ControllerManager`, `CommandReaper`) and of content
  alike; `ContentStorages`, and `UnknownStorage` and `Unnamed` of `ContentError`, which were never a
  storage's, go into it. A storage's `destination` is asked by one question,
  `ConfidentialityQuestions::require_storable`, for a workspace, an artifact's version and, from
  step 3, an attachment. The manager's module docs no longer name a `published_content` that does
  not exist. Open 5 decided: a message may carry attachments alone.
