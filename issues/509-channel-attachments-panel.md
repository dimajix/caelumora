# 509 — A channel's attachments can be found only message by message

**Status:** open
**Type:** feature
**Priority:** low
**Area:** User interaction
**Summary:** The files said in a channel - uploaded by users, attached by agents, given by tools - are shown only at the message that carried them, so finding one again means scrolling through the history, and nothing lists them together.

## The problem

Since issue [282](282-message-attachments.md), a message carries attachments, and a tool result
the media its tool gave its model and what it showed it again. The web UI shows each at its message
in `ChatView`, and `GET …/messages/{mid}/attachments/{aid}` downloads it (see
[REST-SERVER.md](../docs/REST-SERVER.md)). What a channel holds together cannot be seen: there is
no route that lists a channel's attachments, and no place in the web UI that shows them. A file said
days ago is found only by paging back through the history until its message comes.

Issue [208](208-generated-artifacts.md) wanted such a list from the start - "all of a channel's
artifacts together in a panel of their own" - and left it open when it was cut with 282 ("if it is
still wanted"). It is split off here, since 208 is about taking in what comes from outside, and this
is about finding what a channel already holds.

## Considerations

### Whether it is wanted

The channel panel shows the session's workspace already ("Files", [WEB-UI.md](../docs/WEB-UI.md)),
which is where files one works on lie. Attachments are what was said, fixed with their message. A
list of them is worth its cost if channels carry many - an external agent that answers with files
(208), tools that render charts -; with a few, the history serves. Deciding this is the first step.

### What it lists

- The attachments of the messages, with their label, media type, size, classification, who said
  them, when, and the message they belong to.
- Whether the media of tool results are listed too: they are what a model was given, not what was
  said to the channel (282, *What is said and what is seen*). Probably not, or apart.
- A modified message lists its attachments as they stand; one taken back lists none.

### Who sees what

The list is the history seen from another side, so it reaches only as far as the user's view of the
history reaches: an attachment of a message the user does not see is not listed, nor counted
([SECURITY.md](../docs/SECURITY.md), [PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md)). It is
answered by the owner that answers the history, not filtered again at the route; the security suite
gets a test for it.

### REST API and web UI

- `GET …/channels/{cid}/attachments` - a page of the channel's attachments, newest first, each with
  the id of its message, so that it is downloaded by the existing route.
- In the channel panel, a section beside "Files" lists them; an entry downloads the attachment, or
  shows an image as the message does, and leads to its message in the chat.
- The list is loaded again when a message with attachments is said, modified or taken back, and when
  the channel is reclassified.

## Done when

- Whether the list is wanted is decided; if not, this issue is `rejected` with the reason.
- Otherwise: a user can list the attachments of a channel as far as their view of its history
  reaches, over REST and in the channel panel, and go from each to its message; the security suite
  covers what is not listed.
- REST-SERVER.md and WEB-UI.md describe it.

## History

- **2026-10-30** — Issue created, split off from issue [208](208-generated-artifacts.md), which
  wanted a panel of a channel's artifacts and left it open when it was cut with issue 282.
