# 35 — The session list only changes when the user acts

**Status:** open
**Area:** User interaction

## The problem

The web UI's sidebar loads the sessions once (`api.getSessions`) and a
session's channels when it is expanded. Nothing updates it afterwards. A
session created, renamed or deleted elsewhere — in another tab, by another
user, by an agent-created session later on — is invisible until the page is
reloaded. The global event stream (`GET /events`) exists and publishes
exactly those changes, and the web UI does not use it at all.

## Why it matters

It is the visible half of a system that is built around events: sessions and
channels are created by other parties and change while someone is looking at
them. A list that is only right at load time teaches users to reload, and it
will be wrong in exactly the situation the product is for — several
participants working in the same session.

## Considerations

- **Depends on [34](34-snapshot-on-stream-connect.md).** The global stream's
  events carry ids only (`SessionCreated(SessionId)`), so a consumer must
  fetch details for each one; and a stream without a snapshot needs the
  current list from somewhere anyway. With 34 the sidebar gets its state and
  its updates from one place.
- **What the sidebar shows** is sessions, their channels, and a channel's
  members; the relevant events are `Session*`, `Channel*`,
  `UserChannelJoined/Left` and `AgentChannelJoined/Left`. Agent and user
  updates matter only through the channels they are in.
- **Selection must survive updates**: a session being deleted while it is
  open is the case to get right (the UI has `onSessionDeleted` for the
  explicit delete, nothing for a deletion elsewhere).
- **One stream or several?** The UI would then hold two `EventSource`s, one
  global and one per open channel. That is fine, but both should go through
  the same reconnect and session handling ([31](31-session-renewal-from-ui.md),
  [33](33-sse-outlives-its-token.md)) rather than growing their own.
- **Filtering** by what the user may see is [11](11-agent-ownership.md)'s
  question; until it is answered the sidebar shows everything, as it does
  today.

## Done when

- The sidebar reflects sessions and channels created, renamed or deleted
  elsewhere without a reload.
- A session that disappears while it is open leaves the UI in a defined
  state.

## History

- **2026-09-22** — Split off from the discussion of
  [34](34-snapshot-on-stream-connect.md): the snapshot makes the global
  stream usable for a client, and using it in the sidebar is a UI change of
  its own.
