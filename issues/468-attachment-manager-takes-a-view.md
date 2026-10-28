# 468 — The AttachmentManager takes the view of a channel where it needs its coordinates

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `AttachmentManager::create` and `copy` take a `ChannelView`, a layer of access, where an interface should speak of coordinates and properties, and so make every caller reach the channel through the views first.

## The problem

```rust
pub async fn create(&self, channel: &ChannelView, name: &str, classification: Option<Classification>, (size, content): (u64, ByteStream)) -> AttachmentResult<ContentRef>
pub async fn copy(&self, channel: &ChannelView, attachment: &ContentRef) -> AttachmentResult<ContentRef>
```

(`caelumora-runtime/src/attachment/manager.rs:125`, `:208`, as issue
[282](282-message-attachments.md) laid them out). Of the view, the manager reads:

- **who uploads**: the caller of the view's context (`channel.views().context().caller_id`,
  `uploader`, `:45`), and the context itself, which the flow is asked in
  (`require_publish`, `require_storable`);
- **where**: the channel's id (`channel.id()`), the key space and the channel an upload waits for;
- **properties of the channel**: its classification (`channel.classification()`), the default
  of an upload and what it may flow into; and whether the uploader takes part
  (`channel.lists(by)`, `NotAParticipant`).

A view is a layer of access: what a viewer reaches of something, reached in their name. An
interface of a manager speaks of coordinates and properties - that is what the maintainer said
while [453](453-artifact-publish-takes-eight-parameters.md) was done as step 0b of
[467](467-projects-hold-what-outlasts-a-session.md), where `ArtifactManager::publish` was first
given the channel's view after this pattern, and then the channel's address
(`Address<Channel>`) instead. `create` and `copy` are the pattern that was followed there, and
the one place left where a manager takes a view.

## Why it matters

- Every caller has to reach the channel through the views before it can upload: the REST route
  (`caelumora-rest-server/src/routes/attachments.rs:37`, `channel_of`), `attachment_tool`
  (`caelumora-agents/src/tool/attachment_tool.rs:132`), the tests. What the view guarantees - that
  the caller reads the channel - is asked as a side effect of building it, not by the operation.
- The view binds the manager to the views, which depend on the managers in turn.
- Two ways of naming where something is kept in a channel: the attachments by view, the
  artifacts by address.

## Considerations

- **A. Coordinates and properties from the caller.** `create(ctx, channel: Address<Channel>,
  classification of the channel, …)`, and whether the uploader takes part answered by the
  caller. The caller vouches for the properties, as `publish` has the caller vouch for a
  classification (issue 296). Many parameters; the participation check moves out of the owner
  of the uploads, which "one owner per concern" argues against.
- **B. Coordinates from the caller, the properties read by the manager**, as the
  `WorkspaceManager` does it since issue [296](296-workspace-operations-on-the-view.md): "the
  manager does not take the session and the channel from the caller - a frontend has no
  documents -, but reads them itself and asks as the views ask" (`terminal_in`, `shell_in`,
  `kill_command` by ids). The `AttachmentManager` would take the `ControllerManager` or the
  running sessions to read the channel's classification and participants, and ask whether the
  caller reads the channel itself (`AccessQuestions::channel_reading`), so that what is asked
  stays what the views ask today. The pattern exists; the manager takes services it does not hold
  now.
- **C. Leave it**, and say in `CODING-GUIDELINES.md` why a manager of what a channel holds takes
  its view - reached in the caller's name, with the properties it needs -, as a deliberate
  difference. Then `publish` would be the exception, which it is not by its reasons.

Recommendation: B, which follows the pattern the `WorkspaceManager` was given for the same
question; and the rule that a manager's interface speaks of coordinates and properties, never of
a view, written into `CODING-GUIDELINES.md` with it, since the maintainer's correction states no
document yet.

## Done when

- `AttachmentManager::create` and `copy` take the channel by its address; nothing in a manager's
  interface takes a view.
- The security suite asks the same questions as before, through the manager instead of the view.
- The rule is written down.

## History

- **2026-10-28** — Opened at the maintainer's request during step 0b of issue 467, where
  `ArtifactManager::publish` was given the channel's view after the pattern of
  `AttachmentManager::create`, and the maintainer preferred coordinates: a view is a layer of
  access, an interface speaks of coordinates and properties.
