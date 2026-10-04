# 218 — Histories are returned whole

**Status:** open
**Area:** User interaction

## The problem

`GET …/messages` returns a channel's whole history, and since
[207](207-agent-history-in-ui.md) `GET …/actors/{aid}/history` returns an agent's whole history.
Only the channel's stream snapshot is cut: it carries the last 20 messages and `has_more`, and the
web UI then fetches the rest over REST - all of it. Both histories grow with the channel, without a
limit.

## Why it matters

- A long channel makes every reload of the chat, and every opening of an agent's history panel,
  fetch and render all of it, though only the end is shown first ("show earlier").
- The two endpoints are built alike today; paging only one of them would part them.

## Considerations

- **Paging both alike:** `?before=<message id>&limit=n`, answering the messages before that one
  (the last ones without `before`) and whether there are older ones. The web UI fetches older ones
  when "show earlier" is clicked, in the chat and in the history panel.
- **Where it is cut:** the views hand out whole histories (`ChannelView::history`,
  `AgentContextView::history`); cutting them belongs there, not in the REST handlers, so that the
  snapshot's cut (`recent_messages` in `routes/channels.rs`) moves there too.
- A history is read from the store as a whole either way; paging saves transfer and rendering, not
  the read. Reading only part of it would be a matter of the stores.

## Done when

- Both histories are returned in pages, the same way, and the web UI loads older messages on
  request.
- REST-SERVER.md and WEB-UI.md say so.

## History

- **2026-10-04** — Issue created while working on 207: its history was to be paged "like the
  channel's", which is not paged either; the maintainer decided to return both whole for now and
  page them together here.
