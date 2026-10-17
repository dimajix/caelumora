# 404 — The channel overview does not say where something new was said

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The channel list shows only each channel's name, so a user does not see in which channels new messages arrived since they last looked, nor where someone `@`-mentioned them, without opening every channel.

## The problem

The web UI lists the channels of each session in the sidebar
(`caelumora-web-ui/src/components/Sidebar.tsx`): per channel its name, its summary as a tooltip and
a menu - nothing about what happened in it. A user with several channels open in parallel - agents
working in one, a colleague asking in another - has to open each channel to find out whether
anything new was said, and a question addressed to them with `@name` waits until they happen to
look.

What a user would like to see per channel:

*   how many new messages there are since they last read it;
*   whether one of them mentions them.

## Why it matters

Mentions of users are resolved and stored with every message (`Message::mentions`, issue
[29](29-mentions.md)) - for a user a mention only marks them, it asks them nothing. The mark is
only of use if the user notices it where they are not looking. And with agents that work for
minutes, the user switches channels while waiting; the overview is where they would see that an
answer has come.

## What is there today

*   **No read position.** Nothing in the model, the runtime, the stores, the REST API or the UI
    knows up to where a user has read a channel. The nearest is `HistoryLimit` /
    `Channel::history_start_of`, where a participant's visible history begins - a start, not a
    read marker.
*   **No news of messages outside the open channel.** The global stream (`GET /events`) carries
    for a channel only its changes (created, updated, started, joined, ...), no messages; the
    channel's stream (`.../channels/{id}/events`) is opened only for the channel on screen
    (`ChatView.tsx`). `ChannelResponse` has no field for unread messages or mentions.
*   **No count.** The history is a chain with dense positions (`seq`), but `HistoryStore` offers
    pages, single messages and entries from a position - no count. Positions are taken by entries
    that are no messages too (turn ends, reclassifications, changes).
*   **Not everything is readable.** What a user may read of a channel is decided per user
    (`ChannelReading`, issue 133): `ChannelView::history_page_of` starts at `visible_from` and
    passes only what `may_read_item` admits. A count must not count what the user cannot read - nor
    tell by a number that something they may not read was said.

## Considerations

### Where the read position is kept

*   **(a) In the browser** (`localStorage`, as the drafts are). No change on the server. But per
    device and per browser: what was read on the laptop is new again on the phone, and the TUI
    knows nothing of it.
*   **(b) On the server, per user and channel.** Agents have their `AgentChannelState` in the state
    store ([STORAGE.md](../docs/STORAGE.md)); a user's position in a channel is the same kind of
    thing - a participant's state in a channel - and a `UserChannelState` beside it treats users
    like agents. Every frontend sees the same.

Decided: (b). The position is the user's own: nobody else can read it - no read receipts for
others, which would be a feature of its own with its own privacy question.

What the position is: the position in the history of the last message shown, not a message id - a
count and a page both start from a position (`HistoryStart::from` converts where needed).

### When it moves

*   **(a) The frontend says what was read**, e.g. `PUT /sessions/{id}/channels/{cid}/read` with the
    position, when the newest messages were on screen. It moves only forward; an older position is
    ignored, so that two tabs do not move it back.
*   **(b) The server moves it on delivery** to the user's channel stream. But delivered is not read:
    a tab open in the background would mark everything as read.

Decided: (a). An action in the user's name, which needs no permission beyond reading the
channel and comes with its test in the security suite (a user moves only their own position, and
only within what they may read).

Where nothing was read yet - a channel the user never opened - the count starts where their visible
history starts (`visible_from`), capped (below).

### What counts

*   only messages said (`Said`) that the user may read, after the read position;
*   not their own, not internal steps (`Message::internal`, issue 258), not revoked messages;
*   a message changed later (issue 234) is not new again.

A mention counts if one of these messages names the user in `mentions`. A changed message's
mentions are its current ones.

`@all` / `@channel` (issue [401](401-mention-everyone.md)) are expanded into the ids of everyone
present when the message enters, and no marker is kept - 401 decided against one. So a mention by
name and one through `@all` cannot be told apart afterwards. Options:

*   **Both count as a mention.** `@all` addresses the user too; chats commonly notify for it.
*   **Only the explicit one counts.** Needs the marker that 401 rejected - reopening that decision,
    with its cost (a second representation of who was mentioned).

Decided: both count; whoever writes `@all` wants everyone's attention.

### How it is counted

*   **(a) On request**, by reading from the read position forward through what the user may read,
    with a cap (e.g. 99, shown as "99+"). Nothing is kept twice. Cost per channel: the entries after
    the position, at most up to the cap - small for the JSON store (segments of 256 positions),
    cheap for the planned Postgres store (`history_items`, see
    [STORAGE.md](../docs/STORAGE.md)), which can count in the database. The `HistoryStore` gets a
    method that counts with the same `keep` filter as `page`, so that the view's read rules are
    applied once.
*   **(b) Counters kept** per user and channel, incremented as messages are said. Cheap to read, but
    a second bookkeeping beside the history that has to follow reclassifications, revocations and
    changes of what a user may read - every rule of `may_read_item` a second time.

Decided: (a). The count is the `ChannelView`'s: it already knows what its reader may read.

### How the overview learns of new messages

The overview needs to change without the channel's stream being open.

*   **(a) The channel in the global stream changes for the user.** When a message is said, every
    subscriber who may read it is told that the channel has news - a change (e.g. `said`) with no
    content, carrying the reader's new count and whether they are mentioned, computed per
    subscriber with the same rules as the count. The global stream is filtered per subscriber
    already (`EventBus::subscribe_as`); here the filter must also apply the message's
    classification, not only the channel's readability.
*   **(b) A change without numbers**, after which the UI fetches the channel again
    (`GET /sessions/{id}`). Simpler per event, but one request per message per user and channel.

Decided: (a), and `ChannelResponse` (snapshot and `GET /sessions/{id}`) carries the same two
values - `unread` (capped) and `mentioned` - for the user who asks, so that the overview is right
from the start. For a channel the user has on screen, the UI moves the position as messages arrive,
so its count stays at 0.

### Web UI

*   The sidebar shows, per channel, the count as a badge and a mark (e.g. `@`) when the user is
    mentioned; a session that is collapsed shows the sum of its channels, and the mark if any has
    one.
*   The chat view moves the read position when the newest messages are on screen, and when the user
    scrolls down to them.
*   Optional, later: the title of the browser tab with the total.

## Done when

*   A user's read position per channel is kept on the server and moved by the frontend; it moves
    only forward and is visible to that user alone.
*   `ChannelResponse` carries for the asking user the number of new messages (capped) and whether
    one of them mentions them, counted only over what they may read.
*   The global stream tells a user when a channel has news for them, without content and only for
    messages they may read.
*   The sidebar shows the count and the mention mark; opening a channel and reading clears them.
*   The new action has its test in the security suite.
*   [CHANNELS.md](../docs/CHANNELS.md), [REST-SERVER.md](../docs/REST-SERVER.md),
    [STORAGE.md](../docs/STORAGE.md) and [WEB-UI.md](../docs/WEB-UI.md) describe it.

## History

2026-10-17: Opened.

2026-10-17: Decided by the maintainer, as recommended: the read position is kept on the server per
user and channel (`UserChannelState`) and moved forward by the frontend; only readable messages
said after it count, on request and capped; a mention by name and one through `@all` both count;
the global stream tells each reader of news with their count and whether they are mentioned, and
`ChannelResponse` carries both.
