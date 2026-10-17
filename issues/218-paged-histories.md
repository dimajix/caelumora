# 218 — Histories are returned whole

**Status:** done
**Area:** User interaction
**Epic:** [378](378-resource-management-epic.md)
**Summary:** A channel's history and an agent's history are returned whole over REST, though both grow without a limit and only their end is shown first.

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
- **2026-10-16** — Taken up as step 2 of the epic [378](378-resource-management-epic.md), on the
  positions of [376](376-histories-held-whole-in-memory.md). Decided with the maintainer:
  - **A page is addressed by the chain position** (`seq`) of its oldest item, not by a message id -
    which items that are no messages do not have - nor by the item number of
    [379](379-history-stored-as-it-stands.md), which does not exist yet. The server hands it out as
    `earlier`, and a client passes it back as `?before=` without computing with it; 379 is to switch
    pages to the item number.
  - **The cut is in the model and the views:** `History::items_before` reads a page backwards over
    the active path from a position, from where a participant's view begins on
    (`HistoryStart::position_in`), of the items a filter keeps - the messages released, to a viewer
    reading entry by entry - and folds it from its first item to the end.
    `ChannelView::history_page` and `AgentContextView::history_page` hand pages out, the stream's
    snapshot takes its last page from there, and `recent_items` in the REST handlers is gone.
    `history()` stays whole for the tool reading another channel, an agent catching up and
    `ChannelView::member`.
  - **The REST API:** `?before=&limit=` (50, at most 200) on both histories, answered as `{items,
    earlier}`; the snapshot carries `earlier` in place of `has_more`. New: `GET …/messages/{mid}`,
    the message as it stands where the user sees it - what the chat reads back after a change of it
    was not let in, rather than the whole history.
  - **The web UI** loads the page before those shown on request, in the chat and in the agent's
    history panel. A reclassification of the channel shows the chat's last page anew and lets go of
    the older pages loaded; the history panel, reloaded whenever the history grew, reads its pages
    again back to the oldest it shows.

  `Chain::before`, which nothing used, gave way to `Chain::entries_before`, over the active path.
  Whether an agent's message has a recorded context its item now says (`HistoryEntry::context`),
  rather than a search of the history per message; whether it was passed on to the transcript is
  still searched for, from the end backwards. The security suite has S4 for reading in pages and by
  message.
- **2026-10-16** — Done: both histories are returned in pages and the web UI loads older ones on
  request; REST-SERVER.md and WEB-UI.md say so. Merged into `main`.
- **2026-10-16** — [379](379-history-stored-as-it-stands.md) kept the chain position (`seq`) as the
  address of a page: with the derived items addressed by it, a page costs the page, and the item
  number `n` decided on above was not built. The REST API is as this issue left it.
