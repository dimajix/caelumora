# 535 — An image a tool gave and its agent attached is kept and shown twice

**Status:** open
**Type:** question
**Priority:** medium
**Area:** Tools and approvals
**Summary:** An image a tool generates is kept as what its result gave, and once the agent shows it to the channel by `attach` it is kept again as a copy and shown twice in the chat - at the card of the call and at the answer -, so it is unclear whether what a tool gives is rightly seen rather than said.

## The problem

Since issue [512](512-comfyui-image-tool.md), a tool may give its model media: an image
`image_generation` generated lies in the channel's attachments (`AttachmentManager::create_media`)
and is referred to by the result (`ToolCallResult::media`). It is seen, not said (issue
[282](282-message-attachments.md), *What is said and what is seen*): the model is given it, and the
card of the call shows it in the chat (`ToolCallView`). To show it to the others, the agent calls
`attach` (see [TOOLS.md](../docs/TOOLS.md#attachments)), which puts aside a copy for its answer
(`AttachmentManager::copy`): the same content under an id and a key of its own, so that the answer
holds its own attachment.

An agent that does what it is told - generate the image, then show it - so leaves two things:

- **Kept twice.** The content lies under two keys of the storage, one for the result and one for
  the answer. The local storage makes the second a hard link where the file system can
  (`LocalContentStorage::copy`), so that the bytes lie on disk once; otherwise, and in every
  storage that copies, they lie there twice.
- **Shown twice.** The user sees the image at the card of the call and again at the answer.

## Why it matters

Generating images is the case in which a channel carries most files, and in it every image is
doubled - in the chat at once, in the storage where no hard link is made. Issue
[509](509-channel-attachments-panel.md) lists only what was said, so it lists the attached copy
and not what the tool gave; whether that is right depends on this question.

## Considerations

The copy is deliberate: an attachment is part of its message and nothing is counted (282, *How
attachments are managed*) - a message erased or a channel purged removes what it holds, and no
other message refers to it. Whatever is chosen keeps that, or says what replaces it.

- **A. As it is, the card shows less.** The card of a call no longer shows what the answer
  attached, or folds it away. The display is fixed, the storage is not; the web UI has to know which
  attachment is a copy of which media - which nothing records today.
- **B. What a tool gives is said, not seen.** The image becomes an attachment of the turn's answer
  at once, as `attach` would put it aside, and the result names it for the model. Nothing is
  copied and nothing is shown twice; but an agent can no longer generate an image without showing
  it: a draft it throws away, an image only meant to be looked at by the model.
- **C. The answer refers to the result's media.** `attach` refers to the content the result
  gave instead of copying it. Nothing is doubled, but two messages share one content: erasing the
  result's message (issue [237](237-erasing-a-message.md)) or modifying the answer has to know of
  the other - the counting 282 rejected.
- **D. The storage shares, the message does not.** The model stays as it is; every storage makes
  `copy` cheap - a hard link, a server-side copy of S3 (issue [272](272-artifacts-in-s3.md)) -, so
  that only keys are doubled. Fixes the storage only, and only as far as the backend can; the chat
  still shows two images.

## Done when

- It is decided whether what a tool gives stays seen, and how an image shown to the channel is kept
  and displayed once.
- The chat shows such an image once, and the storage holds its content once where the decision
  says so.
- TOOLS.md and CHANNELS.md describe it; the decision is recorded here.

## History

- **2026-11-03** — Issue created, from the work on issue
  [509](509-channel-attachments-panel.md), whose list of attachments shows only what was said.
