# 549 — The kernel takes the agent side's word on a reference to content

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** Identity and access management
**Summary:** `ActorRuntime::load_attachments` and `copy_attachment` take a `ContentRef` from the agent side and act on its key and its label, although a library of the agent side is trusted for nothing, so that it can read content of another channel or copy an attachment under a lower label.

## The problem

[SECURITY.md](../docs/SECURITY.md) says that a library of the agent side is trusted "for nothing",
and that the kernel checks every call of the interface it gives that side. A `ContentRef` that
crosses this interface is what the agent side says it is. Two calls believe it:

- **`ActorRuntime::load_attachments`** (`AttachmentManager::load`,
  `caelumora-kernel/src/attachment/manager.rs:427`) reads the content of the references on the
  messages it is handed and asks nothing, on the reasoning "what lies in an agent's transcript, it
  heard". The transcript is what the agent side hands over, too. No check says that the reference
  lies in the agent's channel: size and digest must fit, which guards against altered content, not
  against somebody else's content whose reference the library holds.
- **`ActorRuntime::copy_attachment`** (`AttachmentManager::copy`, `manager.rs:392`) checks only that
  the key lies in the channel (`lies_in`), and gives the copy the label of the reference it was
  handed (`..attachment.clone()`). A reference with a lower label yields a copy with that label,
  which the channel then admits by that label: the content has left its classification.

The admission of a tool's result has the same shape: what a result gave or showed its model is
checked by `lies_in` alone (`caelumora-kernel/src/channel/run.rs:671`), and its label is the one the
message states.

## Why it matters

Exposure needs a library that misbehaves and a key it knows. The keys are not secret to it: an agent
holds the references of everything it heard. Nothing is exploited by the libraries of this
repository, which pass on what the kernel gave them. But the rule in SECURITY.md exists for the
libraries that are not, and these calls do not keep it.

## Considerations

- **Recognise the reference.** The kernel compares the reference whole with one it knows, as `claim`
  compares an upload (`manager.rs:345`): one a message of the channel carries. The label and key
  then come from the kernel's copy, and the agent side only names an id. The cost is a read of the
  history on a path that runs before every model call, for `load_attachments`, where the
  `ContentCache` is meant to make this cheap.
- **Name the attachment instead of handing it over.** `copy_attachment` takes the message and the
  attachment's id, and reads it through the view as the attachment tools do already
  (`ChannelView::attachment`). It has that view's checks for free, and needs no comparison. For
  `load_attachments` the same means the kernel loads the transcript's messages itself.
- **State the trust.** The agent side's transcript is declared the kernel's input by design, and
  SECURITY.md says so for these calls. It removes no gap, it only names it.

`AttachmentManager::copy` looks like the smaller of the two: its one caller has the message and the
id at hand. How the labels of a result's `media` and `shown` should be checked goes with
`load_attachments`, since both read what the agent side wrote about its own transcript.

## Done when

- `copy_attachment` makes a copy with the label the kernel knows for that attachment, whatever the
  agent side passes, and a test in the security suite holds it.
- `load_attachments` gives a model no content that does not lie in the agent's channel, and a test
  holds it.
- The admission of a result's `media` and `shown` is decided along the same line, or SECURITY.md
  says why it stands apart.

## History

- 2026-11-08: Opened from the review of issue [547](547-long-page-kept-whole-as-media.md), where
  letting the view read a reference handed in by the agent side was rejected for this reason.
