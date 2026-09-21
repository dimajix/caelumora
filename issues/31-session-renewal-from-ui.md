# 31 — An open web UI loses its session after 15 minutes without requests

**Status:** open
**Area:** Identity & access management

## The problem

The session token from [10](10-user-authentication.md) is renewed by the
`require_auth` middleware, but only when a request arrives. Server-sent
events do not count: the SSE stream is one long response, authenticated when
it connects, and it cannot set a new cookie afterwards. A user who has a
channel open and only *reads* — watching agents talk, waiting for a long tool
run — makes no request at all.

After `auth.token_lifetime_secs` (15 minutes by default) the token has
expired. The next action, typically sending a message, gets a `401`, and the
UI switches to the login page. What was typed into the input is lost.

## Why it matters

- It hits exactly the case the product is built for: long-running agents
  whose output the user watches rather than drives.
- Losing a typed message on re-login is the kind of failure that makes people
  distrust a tool, far out of proportion to its cause.
- Raising the lifetime is the wrong fix. The short lifetime is what bounds
  how long a revoked user or a copied token stays valid (see 10); trading that
  away for convenience undoes the reason it was chosen.

## Considerations

### What "idle" should mean

The current behaviour defines idle as "no HTTP request". Any fix picks
another definition:

- **Tab open.** The UI renews on a timer while it is open. Simple, and the
  session then lasts until `max_session_secs` (12 hours) as long as a tab is
  open somewhere — including one forgotten on an unlocked machine.
- **Tab visible.** The same timer, but paused while the page is hidden (Page
  Visibility API), with an immediate renewal when it becomes visible again.
  Closer to "someone is looking at it", and costs nothing extra.
- **User active.** Renewal only after real interaction (input, clicks,
  scrolling) within the last N minutes. The most honest meaning of idle, but
  "watching agents" is not interaction, so it does not solve the case above
  by itself.

"Tab visible" looks like the right default; it is the one that keeps a
watched session alive without keeping an abandoned one alive.

### How to renew

- `GET /auth/me` already passes through the middleware and so renews when
  due. A timer at about a third of the lifetime is enough to never miss the
  renewal window. The UI does not know the lifetime; the server could send it
  with the login and `me` responses, or the UI could use a conservative fixed
  interval.
- A dedicated `POST /auth/refresh` would make the intent explicit in logs
  and could renew unconditionally rather than only past half the lifetime.
  Not needed for the mechanism, possibly nicer for reading logs.
- Renewing *over* SSE is not possible: headers are sent once, at the start of
  the stream.

### Not losing work anyway

Whatever renewal does, a `401` can still happen: after `max_session_secs`,
after a server restart without a configured signing key, after the user was
removed. The input draft should survive that — kept in `sessionStorage` per
channel and restored after login — and ideally the UI returns to the channel
it was on. This is worth doing even with renewal in place.

### Related, but not this issue

An open SSE stream stays open after its token has expired or its user has
been removed, because the stream is only checked when it connects. Closing
streams at the token's `exp` (and letting `EventSource` reconnect, which
re-checks the cookie) would fix that on the server side. It interacts with
renewal — a stream closed at expiry reconnects with whatever cookie the timer
has renewed — so both should be designed together, but it is a server change
and can be done separately.

## Done when

- A channel that is open and visible in the web UI does not lose its session
  while the user only reads, up to `max_session_secs`.
- The definition of idle that results is decided and documented in
  [10](10-user-authentication.md).
- Text typed into the message input survives a forced re-login.

## History

- **2026-09-21** — Issue created after [10](10-user-authentication.md) was
  implemented: token renewal happens only on requests, and SSE traffic does
  not count as one.
