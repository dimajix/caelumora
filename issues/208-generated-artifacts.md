# 208 — Artifacts an agent generates are dropped

**Status:** open
**Type:** feature
**Priority:** high
**Area:** User interaction
**Summary:** Files and data an agent generates - an external agent's artifacts, a tool's images - are dropped; only their text is kept, and nothing can be stored or downloaded.

## The problem

Agents can produce more than text - a document, an image, a table, a file. Above all external
agents ([179](179-external-agents-via-a2a.md)) do: an A2A task's result is a list of *artifacts*,
each made of parts, and a part is text, a file (inline bytes or a URL, with a media type and a file
name) or structured data. Caelumora keeps only the text:

- `a2a_agent` reads of every part only `text` (`WirePart` has no other field, `text_of`); an
  artifact's text becomes an `ExternalEvent::Message`, its files and data are lost without a trace,
  and an artifact without text leaves nothing at all.
- The same holds for tool results: `mcp_tool` keeps only the text contents of a `CallToolResult`
  (`call_tool_result_to_value`), so images and embedded resources an MCP server returns are lost
  too.

There is nowhere to put such a thing either: a `Message` carries text, tool calls and tool results,
and no store keeps binary content.

## What is wanted

- Artifacts an agent generates are **taken in and stored**.
- The user can **download** them.
- They are visible **in the conversation**, at the message that brought them, and **all of a
  channel's artifacts** together in a panel of their own.

## Considerations

### What an artifact is

An artifact of our own: an id, a name (file name), a media type, a size, its content (or where it
is kept), who produced it and in which channel and message, when, and its classification. One A2A
artifact may hold several parts; whether each file part becomes an artifact of ours, or an A2A
artifact maps to one of ours with several parts, is to be decided. Text parts stay text of the
message, as today. Structured data (`data` parts) may be kept as a JSON artifact.

### Where it is stored

Neither store fits as it is: the `EntityStore` keeps documents, the `StateStore` an actor's state,
and both are written whole or as changes of JSON (see [STORAGE.md](../docs/STORAGE.md)). Binary
content of arbitrary size wants a store of its own - an `ArtifactStore` (or blob store) with a
file-system implementation first, an object store later - holding the content by id, with the
metadata kept beside it or in the entity store. Size limits, per artifact and per channel, belong to
it. What happens to a channel's artifacts when the channel or session is deleted has to follow what
happens to its messages.

### How a message refers to it

A `Message` gains references to the artifacts it brought (id, name, media type, size), not their
content: messages are hashed into transcript chains (`digest::message_chain`) and sent to every
participant. Whether a reference is part of a message's digest is to be decided; it should be,
since the artifact is part of what was said.

### Files given by URL

An A2A file part may carry a URL instead of bytes. Fetching it means a request to an address the
remote agent chose (to internal addresses as well); keeping only the link means the user downloads
from elsewhere and the content may change or vanish. Fetching, with the same care as any outbound
request and limited in size, is probably right; at least it must be decided and documented.

### Confidentiality and permissions

An artifact is classified like the message that brought it (the external agent's classification,
issue 133) and may be reclassified with it. Downloading it is reading the channel: whoever may read
the message may download its artifacts (`AccessQuestions`, [SECURITY.md](../docs/SECURITY.md)), and
the security suite gets a test for it. The content is served with its media type but as a download
(`Content-Disposition: attachment`), never rendered inline from our origin unless it is a type known
to be harmless, since an HTML or SVG artifact would otherwise run in the web UI's origin.

### What the agents are given

Whether other agents are told of an artifact - its name and type in their transcript, its text for
a model that can read it, an image for a model that can see - is a question of its own, as is
handing an artifact to an external agent. Not giving it on is the safe first step.

### REST API and web UI

- `GET …/channels/{cid}/artifacts` lists a channel's artifacts (paged), `GET …/artifacts/{id}`
  downloads one; the message carries the references.
- In `ChatView`, a message shows its artifacts as attachments with name, type, size and a download
  button; images may be shown as previews.
- A panel per channel, opened like the notes, lists all its artifacts with their producer and time
  and links each to its message.

### One solution for every producer

Artifacts do not come from external agents only. Tools generate them too: an MCP tool that returns
an image or a resource, and internal tools - built-in ones, or tools of our own an agent run here
calls - that write a file, render a chart or export a table. Both kinds need **one solution**, not
one for A2A and another for tools:

- one artifact type, one `ArtifactStore`, one kind of reference in a message, one permission, one
  REST route and one way of showing them;
- whoever produces an artifact hands it over the same way - the external agent from an A2A answer,
  a tool from its result (`ToolCallResult` gains artifact references beside its text, or a tool is
  given a way to put an artifact into the store and returns its reference);
- the producer is recorded with the artifact: the agent, and for a tool the call that made it.

A2A is the first producer, not the shape of the design.

## Done when

- File and data parts of an external agent's answer, and files and images of tool results - MCP
  tools and internal tools alike - are kept as artifacts through one mechanism, stored, and
  referred to by the message or tool result that brought them; nothing is dropped silently any
  more.
- A user who may read the message can download its artifacts; the security suite covers it.
- The web UI shows artifacts at their message and in a panel per channel.
- STORAGE.md, CHANNELS.md (external agents), TOOLS-AND-COMMANDS.md, REST-SERVER.md and WEB-UI.md
  describe it.

## History

- **2026-10-03** — Issue created. Found: `a2a_agent` and `mcp_tool` keep only text parts today;
  everything else they receive is dropped.
- **2026-10-08** — The workspace exists (issue [253](253-workspace.md), with 251): named files
  of a user's, in a configured storage, one classification for all of them, used in a session
  through its channels, uploaded and downloaded over REST - a download always an attachment,
  `application/octet-stream`, `nosniff`, never shown inline - and in the web UI. PLAN.md's
  recommendation for this issue becomes concrete: an artifact a remote agent or tool generates is a
  file written into the session's workspace, where the channel uses one, and the message refers to
  it by workspace and path, with the digest of its content. What becomes of an artifact in a
  channel without a workspace, and whether a workspace is made for it, is still to be decided.
- **2026-10-09** — Where artifacts are kept is decided by issue
  [271](271-publishing-artifacts.md), against the recommendation above: not as files of the
  session's workspace, but as artifacts of their own - an `Artifact` document in the entity store,
  its content in an `ArtifactStorage`, in a scope of its own -, published there as fixed copies.
  That also answers what becomes of an artifact in a channel without a workspace. What remains here
  is taking in the file and data parts of A2A answers and MCP results as such artifacts.
- **2026-10-24** — Cut with issue [282](282-message-attachments.md), decided there (*What is said
  and what is seen*): what an agent generates is not an artifact any more, but an attachment or
  what a model was given. 282 brings the type of reference, `ToolCallResult::media` - what a tool
  gave its model, the result's label and visibility, not heard by other agents - and `shown`, the
  way through `ToolOutput` and the `ToolExecutor`, which stores what a tool hands over as bytes,
  and `attach`, by which an agent shows the channel what it chooses. What remains here is taking in
  what comes from outside: an MCP result's `image` and `resource` contents as media - which also
  ends `mcp_tool` putting a result without text, base64 and all, as JSON into the context
  (`call_tool_result_to_value`) -, an A2A answer's file parts as attachments of the external
  agent's message, and the question of files given by URL. The panel of a channel's artifacts
  wanted above would become a list of a channel's attachments, if it is still wanted.
