# 33 — An open event stream outlives the token that authenticated it

**Status:** open
**Area:** Identity & access management

## The problem

`GET /sessions/{id}/channels/{cid}/events` and `/events` are server-sent
event streams: one response that stays open for as long as the client
listens. `require_auth` runs once, when the stream is opened, and never
again — there is no further request to check. A stream therefore keeps
delivering events after its token has expired, after the user was removed
from the user provider, and after that user logged out in another tab.

The session cookie itself is bounded: it expires, and
[31](31-session-renewal-from-ui.md) renews it only while someone is
watching. The stream is not bounded by anything except the client closing
it or the server stopping.

## Why it matters

It is the one place where the bound that [10](10-user-authentication.md)
deliberately put on a non-revocable token does not hold: "a disabled user is
out after at most one token lifetime" is true for every route except the one
that carries the actual conversation.

## Considerations

- **End the stream when the token expires.** The claims are known when the
  stream is opened, so the response can simply end at `exp`. `EventSource`
  reconnects by itself, and the new request is authenticated again — with
  the cookie that [31](31-session-renewal-from-ui.md) has renewed in the
  meantime, so a watching user notices nothing. Without renewal the
  reconnect gets a `401`, which is what the UI already handles.
- **Re-check the user, not just the token.** Ending at `exp` bounds a
  removed user by one lifetime, the same as everywhere else. Checking the
  provider periodically inside the stream would be stricter but puts a
  recurring backend call on every open stream.
- **Reconnect cost.** Every reconnect re-subscribes to the channel's
  broadcast. Events that occur between the two connections are lost —
  `EventSource` has `Last-Event-ID`, which the current stream does not use,
  so a reconnect silently skips whatever happened in between. Ending
  streams deliberately makes that gap regular rather than rare, so it
  should be decided together with whether events get ids.
- **Logout elsewhere** stays unsolved either way for up to one lifetime,
  because the token remains valid; that is the accepted trade of
  [10](10-user-authentication.md).

## Done when

- An event stream does not outlive the token that authenticated it.
- Reconnecting is invisible to a user whose session is being renewed, and
  it is decided what happens to events that occur during the gap.

## History

- **2026-09-22** — Split off from [31](31-session-renewal-from-ui.md), which
  covers renewing the session from the UI; this covers the stream that
  ignores the result.
- **2026-09-22** — Prerequisite identified: ending a stream is only
  acceptable once a reconnect can restore what was missed, which is
  [34](34-snapshot-on-stream-connect.md). The "reconnect cost" consideration
  above is answered there — by a snapshot on connect rather than by event
  ids — so 34 should come first.
- **2026-09-22** — [34](34-snapshot-on-stream-connect.md) is done: a
  reconnecting client can now ask for a snapshot and lose nothing, so the
  prerequisite for ending streams at a token's expiry is in place.

