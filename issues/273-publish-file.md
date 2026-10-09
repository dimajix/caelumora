# 273 — An agent cannot point a user to a file of the workspace to download

**Status:** open
**Area:** Tools and approvals

## The problem

An agent that has made a result in the session's workspace - a report, an export, an image - can
only say in text where the file lies. The user has to open the workspace's file browser and find
it there. There is no way for the agent to offer a file deliberately, as a download at the message
that hands it over.

Issue [271](271-publishing-artifacts.md) decides the durable answer: artifacts as fixed copies in a
store of their own, with a scope and a lifetime independent of the workspace. That is a larger
piece of work - a document, a storage, a manager, REST routes, a panel. Until it exists, a much
cheaper step already gives most of the value.

## Why it matters

Handing over a result is the end of most work an agent does in a workspace. A download at the
message that offers it is what a user expects, as claude.ai offers the files it makes in its
sandbox as downloads in the chat; its artifacts - versioned documents that are shown, shared and
published - are a thing of their own, as 271's artifacts will be here.

## Considerations

**A link, not a copy.** `publish_file` of `terminal_tool`, beside the file tools, offers a file of
the session's workspace as it lies there:

- parameters: the file's `path`, and optionally a `name` to offer it under (its file name unless
  given) and a short `description`;
- it checks that the file exists and is a file, and answers a reference: the workspace, the path,
  the name, the size and the digest of the content at the moment of publishing;
- it changes nothing, so it records no change; it reads the file to take its digest, which is
  recorded as the turn's knowledge as `read_file` records it;
- whether it needs approval: it hands nothing to anyone who could not download the file through the
  channel already (below), so it is a read - not asked under `"unless_read_only"`.

**Download through the route that exists.** The web UI downloads it through the channel's
workspace (`GET …/channels/{cid}/workspace/files/{path}`), with that route's permission: a current
member, the channel using the workspace, it and the channel flowing both ways. No new route, no new
permission, nothing in the security suite beyond what that route has.

**Presented as a download.** A presentation of its own (`ToolPresentation::FilePublish`, after
issues [265](265-file-list-and-search-presentations.md) and [267](267-move-file.md)): a card with
the name, the size, the description and a download button. Since the link points to the file as
it is *now*, the card compares on demand - or when the channel's files change - and says so: the
file was changed since it was offered (its digest differs), or it is gone. A user who cannot use the
workspace in the channel sees the card without the button, and why.

**What it does not do**, and 271 will:

- keep what was offered: a file changed after publishing is downloaded as it is now, a file moved
  or deleted is gone;
- outlive the workspace, or be downloaded by someone who may read the channel but not use its
  workspace - the session's owner who is no member, say;
- keep a label of its own, a scope, versions.

**Built to be kept.** The tool's name, its parameters, its presentation and the card stay when 271
arrives; what changes is what lies behind them: publishing then copies the file into an artifact,
and the card downloads the artifact. The model's contract - "offer this file for download" - does
not change. Results of calls made before then may be deleted rather than read in their old form
(AGENTS.md).

## Done when

An agent offers a file of the workspace for download with one call; the call is shown as a card
with a download that works for whoever may use the workspace in the channel, and says when the
file changed or is gone; tests, `TOOLS-AND-COMMANDS.md`, `COMPONENTS.md` and `WEB-UI.md` say so.

## History

- **2026-10-09** — Opened at the maintainer's request while deciding 271: a cheap `publish_file`
  first, a link to a file of the workspace; artifacts, with their own store and versions, come
  with 271 and take its place behind the same tool.
