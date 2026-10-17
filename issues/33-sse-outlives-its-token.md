# 33 — An open event stream outlives the token that authenticated it

**Status:** done
**Area:** Identity and access management
**Summary:** An open server-sent event stream is authenticated once, when it connects, and keeps delivering after its token has expired or its user was removed.

## The problem

`GET /sessions/{id}/channels/{cid}/events` and `/events` are server-sent event streams: one response
that stays open for as long as the client listens. `require_auth` runs once, when the stream is
opened, and never again — there is no further request to check. A stream therefore keeps delivering
events after its token has expired, after the user was removed from the user provider, and after
that user logged out in another tab.

The session cookie itself is bounded: it expires, and [31](31-session-renewal-from-ui.md) renews it
only while someone is watching. The stream is not bounded by anything except the client closing it
or the server stopping.

## Why it matters

It is the one place where the bound that [10](10-user-authentication.md) deliberately put on a
non-revocable token does not hold: "a disabled user is out after at most one token lifetime" is true
for every route except the one that carries the actual conversation.

## Considerations

- **End the stream when the token expires.** The claims are known when the stream is opened, so the
  response can simply end at `exp`. `EventSource` reconnects by itself, and the new request is
  authenticated again — with the cookie that [31](31-session-renewal-from-ui.md) has renewed in the
  meantime, so a watching user notices nothing. Without renewal the reconnect gets a `401`, which is
  what the UI already handles.
- **Re-check the user, not just the token.** Ending at `exp` bounds a removed user by one lifetime,
  the same as everywhere else. Checking the provider periodically inside the stream would be
  stricter but puts a recurring backend call on every open stream.
- **Reconnect cost.** Every reconnect re-subscribes to the channel's broadcast. Events that occur
  between the two connections are lost — `EventSource` has `Last-Event-ID`, which the current stream
  does not use, so a reconnect silently skips whatever happened in between. Ending streams
  deliberately makes that gap regular rather than rare, so it should be decided together with
  whether events get ids.
- **Logout elsewhere** stays unsolved either way for up to one lifetime, because the token remains
  valid; that is the accepted trade of [10](10-user-authentication.md).

## Done when

- An event stream does not outlive the token that authenticated it.
- Reconnecting is invisible to a user whose session is being renewed, and it is decided what happens
  to events that occur during the gap.

## History

- **2026-09-22** — Split off from [31](31-session-renewal-from-ui.md), which covers renewing the
  session from the UI; this covers the stream that ignores the result.
- **2026-09-22** — Prerequisite identified: ending a stream is only acceptable once a reconnect can
  restore what was missed, which is [34](34-snapshot-on-stream-connect.md). The "reconnect cost"
  consideration above is answered there — by a snapshot on connect rather than by event ids — so 34
  should come first.
- **2026-09-22** — [34](34-snapshot-on-stream-connect.md) is done: a reconnecting client can now ask
  for a snapshot and lose nothing, so the prerequisite for ending streams at a token's expiry is in
  place.
- **2026-09-23** — Implemented. Decisions:
  - **A stream ends five seconds before its token does.** `require_auth` hands the expiry of the
    token the request leaves with - the renewed one, if it was renewed - to the handler as
    `AuthenticatedUser::expires_at`, and `sse::bounded_by_token` ends the response there. The margin
    keeps the reconnect from racing the expiry it was caused by. A token with less than twice the
    margin left keeps its stream to the very end instead: taking the margin off would leave a stream
    so short that the client would reconnect into another one just as short.
  - **The end is announced** by a named `expired` event, so a client can reconnect at once instead
    of waiting out its retry interval and freezing its output meanwhile. A client that does not know
    the event sees the stream simply end, as it would anyway.
  - **Every stream starts with `retry: 3000`,** the SSE field that sets how long a client waits
    after an *unexpected* break. Without it that delay is whatever the browser defaults to (3 to 5
    seconds); with it, it is the server's decision and the same everywhere.
  - **The user is not re-checked inside a stream.** Ending at `exp` bounds a removed user by one
    token lifetime, the same as everywhere else, and the reconnect asks the `UserManager` again. A
    periodic check would put a recurring backend call on every open stream, which
    [27](27-ldap-user-backend.md) would feel.
  - **Streams do not renew the session.** Found while verifying: the reconnect is an ordinary
    request, so `require_auth` renewed on it, and a background tab would have kept itself logged in
    through its `EventSource` alone - see the correction in [31](31-session-renewal-from-ui.md). The
    two stream routes now use `require_stream_auth`, which verifies the token but never renews it.
  - **Events during the gap** are not lost: the web UI reconnects with `?snapshot=true` from
    [34](34-snapshot-on-stream-connect.md) and drops duplicates by message id. `Last-Event-ID` is
    therefore not needed.
  - **The web UI** no longer closes its `EventSource` on every error: while the browser is
    reconnecting by itself (`readyState === CONNECTING`) it leaves it alone, and only a stream that
    is really closed triggers the session check that leads to the login page.
  - Verified against the REST server with a 20-second token lifetime: a stream on a fresh token ran
    15 seconds and ended with `expired`, a reconnect with 5 seconds left ran those 5 seconds, the
    next one answered `401`, and no stream response ever set a cookie. After a `POST /auth/refresh`
    a stream ran its full 15 seconds again. The UI's reconnect was not exercised in a browser.
