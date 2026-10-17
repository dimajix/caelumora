# 417 — On reload a user is shown the channel's history, not what their actor showed them

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** Live, a user sees what the moderator routed to their actor; on reload they see the channel's history, filtered by what they may read, so the two can differ and whatever went to the user alone is gone.

## The problem

Since [220](220-user-stream-from-its-actor.md) a user's stream is what their actor shows: what the
moderator routes to them and what they say themselves, in that order. The reason is in 220: what a
user sees should be what is routed to them, one owner, the moderator, and no second path with a
filter of its own.

What the user is shown on reload, and as the last page of a snapshot, is that second path: the
channel's history (`ChannelView::history_page`), read from the `HistoryStore` under
`HistoryKey::Channel` and filtered by where the user's view begins and by what they may read. The
user's state in the channel (`UserChannelState`) holds only their read mark. So the two differ:

- **What went to the user alone is lost on reload**: a private answer of a command, a chain cut off
  (`ChainCutOff`), a message the channel refused, a retry rejected. WEB-UI.md accepts this as
  "shown live and gone on reload".
- **A moderator that withholds something from a user is undone on reload.** `Moderator::route` may
  deliver a message to some users and not to others; the history shows it to everyone who may read
  the channel. No moderator does this today, since `policy_moderator` delivers every message to
  every user, but nothing rules it out.
- **The order can differ.** A user's own message is shown as they send it, before the channel takes
  it in, so live it may stand before a message the history places earlier.

An agent has a history of its own in every channel (`AgentChannelState`): everything offered to it
and what it said. A user has none.

## Why it matters

- What a user sees should have one source. Live it is the moderator's routing; on reload it should
  be the same, not a reconstruction from another record.
- What a user was told privately - the answer of `/help`, a refusal - should still be there after a
  reload or on another device.

## Considerations

### Decided: the user's actor keeps a history

Three ways were weighed:

- **A**: state that the moderator decides delivery, not what may be read; the channel's history
  stays the one source, and every moderator delivers every message heard to every user. Only
  documentation, but what went to the user alone stays lost.
- **B**: the user's actor keeps a history of what it showed, as an agent's actor keeps its own.
- **C**: one channel history whose items carry an audience, filtered by it on reading.

**Decided: B.** What a user sees is what their actor showed them, live and on reload. Its cost -
another copy of every message per user - is what [416](416-messages-stored-once-per-holder.md)
removes: with messages kept once per form, the user's history holds references to the `published`
form the channel's history holds already.

### What the user's history holds

Everything the actor shows (`Shown`) that is complete: the messages routed to the user and those
they said, every change of them, the marks of turns, steps, commands, activities and of who joined
or left - and what went to the user alone and is not stored today: a private command answer, a
chain cut off, a refusal. Not what streams while a message is under way.

**Recommended:** new variants of `HistoryEntry` for what only a user's history holds, so that there
stays one `HistoryStore`, one rule of folding and one form of a page, with a `HistoryKey::User`.
The alternative, an entry type of its own for users, would need a second store and a second page.

### What stays on the channel's history

- **A reader without an actor** - the session's owner who takes no part - reads the channel's
  history as today.
- **A user who joins with a view of the past** sees the channel's history up to where they joined,
  their own from then on; or their history begins with a copy of the references to what their view
  of the past reaches. To be decided.
- **The read mark** (`ReadMark`) counts positions of the channel's history today; it moves to the
  user's history, and what is new is counted there.
- **A reclassified channel**: what a user may read still follows the channel's classification, so
  the filter by what may be read stays on the user's history too.
- **A former participant** keeps their history, as an agent that left keeps its own.

### What changes for clients

`GET …/history` and the snapshot answer from the user's history for a participant, in the same
form of a page. The web UI keeps what went to the user alone instead of dropping it on reload; WEB-UI.md
and REST-SERVER.md say so.

## Done when

- A user's actor keeps a history in the channel, stored with the user's state.
- A participant's page of the history and the last page of the snapshot are read from it; a reader
  without an actor reads the channel's history.
- What went to the user alone - a private command answer, a chain cut off, a refusal - is there
  after a reload, and only for that user.
- A message a moderator does not route to a user is not on their page.
- The read mark and what is new are counted on the user's history.
- The security suite checks that one user's history is not readable by another.
- CHANNELS.md, STORAGE.md, REST-SERVER.md and WEB-UI.md describe the user's history.

## History

- 2026-10-17: Opened after looking at where a reload reads from. Depends on
  [416](416-messages-stored-once-per-holder.md). B is decided; how a user who joins with a view of
  the past is shown it, and whether what only a user's history holds becomes new `HistoryEntry`
  variants, are to be decided before it is built.
