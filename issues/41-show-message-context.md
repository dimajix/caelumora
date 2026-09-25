# 41 — The web UI cannot show what an agent saw

**Status:** open
**Area:** User interaction

## The problem

Since [06](06-context-in-message-for-auditability.md) the server can answer,
for every message an agent produced, what its model saw when it wrote it:
`GET /sessions/{id}/channels/{cid}/actors/{aid}/messages/{mid}/context`
returns the system prompt of that turn and the messages of its context, with
their digests. Nothing in the web UI uses it. The question the endpoint was
built for - "why did the agent answer like that?" - can only be asked with
a hand-written request.

## Why it matters

Looking at what an agent saw is the first step whenever it behaves oddly,
and the people who notice are the ones reading the chat. An answer that
needs a REST client is an answer most of them will never get.

## Considerations

- **Where to ask:** on an agent's message - an action on the message (an
  icon or menu entry) that opens the context in a side panel or dialog. The
  request needs the message's sender as the actor (`sender_id`), which every
  message already carries.
- **Who sees the action:** under the ownership policy only the agent's
  owner may read its context; other participants get `403
  context_forbidden`. The UI does not know who owns an agent. Either it
  offers the action on every agent message and shows the refusal, or the
  server tells it in advance (e.g. a flag per member or per message) - the
  latter means a policy decision leaks into a listing, and should be
  weighed against simply handling the `403`.
- **When it is available:** the reference is recorded when the agent's turn
  ends. A message still streaming, or shown before its turn finished, has
  none yet (`404 context_not_recorded`); the action should appear, or
  succeed, only for completed messages.
- **How to show it:** the system prompt (collapsible, it is long and the
  same for many messages), then the context's messages in the order they
  were sent - including internal messages and tool calls and results, which
  the chat itself does not show, so they need to be marked as such. The
  digests can be shown short, with a way to copy them, for anyone comparing
  contexts.
- **Size:** a context grows with the conversation. Showing the end first,
  or collapsing older messages, keeps the panel usable; paging on the
  server can wait until a context is too large to send at once.
- **Caching:** a message's context never changes, so the UI can keep what it
  fetched for as long as it runs.

## Done when

- On a completed agent message, the web UI can show the system prompt and
  the messages its model saw, with internal messages and tool steps marked.
- A refusal or a missing context is shown as such, not as an error of the
  UI.

## History

- **2026-09-25** — Split off from [06](06-context-in-message-for-auditability.md),
  which built the endpoint; showing it in the web UI is a step of its own.
