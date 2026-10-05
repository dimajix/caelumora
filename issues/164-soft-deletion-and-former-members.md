# 164 — The store forgets what was deleted and who left

**Status:** open
**Area:** Persistence

## The problem

Deleting a session, a channel or an agent removes its document from the `EntityStore`, and removing
a participant from a channel removes it from the channel's document. Afterwards nothing says that
they were there:

- A memory entry's origin (`MemoryOrigin::channel`) and a `KnowledgeRef` can name a channel that is
  gone; the views answer `None` (`Views::channel_of`, [163](163-navigable-views.md)) - "gone" and
  "never there" are one answer, and what the agent read or remembered cannot be told apart from a
  dangling reference.
- An agent removed from a channel is a former member only by the messages it left in the history
  (`ChannelView::member`, `MemberView::former`, issue 163): one that was removed before it said
  anything, or whose messages the viewer may not see, is not found, though its state in the channel
  is still in the `StateStore`.
- Who deleted or removed what, and when, is nowhere ([07](07-session-deletion-audit.md),
  [140](140-audit-log.md)).

## Why it matters

- Auditability: a deletion is the one operation that destroys evidence (07). What an agent did in a
  channel it was removed from - its tool calls, what it saw - should stay readable to whom it is
  readable, and so should the fact that it was removed.
- The views (163) need to say that an edge leads to something that *was* there, not guess it from
  side effects.

## Considerations

- **Mark, do not remove.** A deleted entity stays in the store with a status (`deleted`, by whom,
  when) and is left out of every listing and refused to every read that is not explicitly about what
  was deleted. Likewise a participant that leaves a channel: `Status: Left` in the channel's
  document, with when and by whom, instead of being dropped from `participants`. The views then list
  current members and reach former ones from the document, and `channel_of` can answer "gone".
- **Tombstone versus content.** 07 separates the record that something existed and was deleted from
  its content, which a user may want removed for good. Marking covers the record; whether the
  content stays is 07's question.
- **Where the mark lives.** In the documents (`Session`, `Channel`, `Agent`, `Participant`), since
  the stores are documents; the `StateStore` keeps the agents' states as it does.
- Every place that deletes - `SessionManager::delete_session`, `SessionLoop::delete_channel`,
  `ChannelLoop::remove_participant`, the entity stores' `delete_*` - and every listing changes with
  it.

## Done when

- Deleting a session, a channel or an agent and removing a participant leave a mark with who and
  when, and nothing is removed from the store.
- Listings and reads leave out what is marked deleted, unless asked for what was.
- The views reach a former member from the channel's document and tell a gone channel from one that
  never was.

## History

- **2026-10-01** — Opened by the maintainer in step 6 of [163](163-navigable-views.md): a `None` for
  a gone channel and a former member known by its messages are what the views can do until the store
  keeps what was deleted. Plays into [07](07-session-deletion-audit.md).
- **2026-10-05** — From [146](146-show-given-up-turns.md): the turn an agent gave up because it
  was removed is recorded, but the web UI offers an agent's history and given-up turns for members
  only, so it cannot be shown once the agent has left - exactly when it would be asked for. A former
  member known to the views would make it reachable.
