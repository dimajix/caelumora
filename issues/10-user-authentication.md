# 10 — User authentication

**Status:** open
**Area:** Identity & access management

## The problem

There is no authentication anywhere in the system. The REST handlers construct
a `SecurityContext` themselves rather than deriving one from a credential, and
the only `SecurityManager` implementation is `DummySecurityManager`
(`caelumora-model/src/security.rs:85`), which is not a policy.

Every caller is therefore whoever they claim to be, with unrestricted rights.

## Why it matters

Nothing else in the access-control area can be built on top of an unidentified
caller. [11](11-agent-ownership.md) needs an owner,
[12](12-user-and-agent-permissions.md) needs a subject to combine permissions
for, and [14](14-approval-privilege.md) is meaningless if anyone can claim to
be an approver. This issue gates all three.

It also gates deployment beyond a trusted single-user machine: the REST server
in its current form must not be exposed to a network.

## Considerations

- The structure to receive the result already exists. `SecurityContext`
  carries `initiator_id`, `caller_id` and `delegation_ids`; the work is to
  populate it from an authenticated credential at the REST boundary instead of
  fabricating it, and to give `SecurityManager` a real implementation.
- The `User` entity already lives in the `EntityStore`, so there is a place to
  hang credentials — though credential material should go through
  `SecretProvider`, not into the entity store as plaintext.
- Token-based (bearer/JWT) is the natural fit for the REST server and costs
  the least in the TUI and local-client paths, where authentication may be
  bypassed entirely by an explicit trusted-context construction. That bypass
  must be a single, obvious, greppable place.
- The `caelumora-local-client` case deserves a decision: it runs in-process
  and has no network boundary. Requiring authentication there is arguably
  pure cost — but so is having two code paths.

### Browser sessions: a JWT in an `HttpOnly` cookie

Decided 2026-09-21 for the web UI. Where the identity comes from, i.e.
verifying a password, is [25](25-file-user-backend.md) and its siblings. This
section covers only how the browser keeps that identity afterwards.

**Flow.** A login page posts `{username, password}` to `POST /auth/login`,
which verifies through the `UserProvider` chain and answers with
`Set-Cookie: caelumora_session=<JWT>; HttpOnly; SameSite=Strict; Secure; Path=/`.
`Secure` is left off only for `localhost` during development.
`GET /auth/me` returns the current user, or `401`; the UI calls it on start
and shows the login page on `401`, and a `fetch` wrapper does the same for any
later `401`. `POST /auth/logout` clears the cookie. An axum middleware reads
the cookie, verifies the token and builds the `SecurityContext` from it,
replacing the one the handlers fabricate today.

**Why a cookie and not an `Authorization` header.**
- `EventSource`, which the UI uses for channel and global events, cannot set
  headers. A bearer token would have to go into the query string and so into
  every access log, or SSE would have to be reimplemented over `fetch`.
  Cookies are sent automatically.
- `HttpOnly` keeps the token out of reach of scripts. The UI renders markdown
  produced by LLMs, so XSS is a real surface; a token in `localStorage` would
  be readable by any injected script.
- The UI already talks to the server on the same origin (`/api`), so cookies
  need no CORS. `SameSite=Strict` plus accepting only `application/json` on
  writing endpoints covers CSRF without a separate token.

**Why a JWT and not a session id with a server-side session store.** The
in-memory store alone would be cheap. The cost arrives with more than one
process ([21](21-scaling.md)): the store would have to move into the
`StateStore`/Postgres ([04](04-postgres-state-store.md)) and be consulted on
every request. With a JWT, processes only need to share the signing key.

**What that costs, and how it is bounded.** A JWT cannot be revoked. Logout
only deletes the cookie; a copied token stays valid, and a user disabled in
the provider stays logged in until the token expires. Accepted deliberately,
bounded as follows:
- **Short lifetime** (on the order of 15 minutes) with **sliding renewal**:
  the middleware issues a fresh token once the current one is past about half
  its lifetime. **Only on renewal** does it ask the `UserProvider` whether the
  user still exists and is enabled. The guarantee is: a disabled user is out
  after at most one token lifetime, at the cost of one provider lookup per
  user every few minutes rather than per request.
- **Absolute limit** through an `auth_time` claim that renewal carries over
  rather than resets, e.g. 12 hours, after which the user must log in again
  however active they are.
- **Minimal claims:** `sub` (`UserId`), `iat`, `exp`, `auth_time`. Roles are
  not put into the token but read from the provider on renewal. Otherwise
  there would be two places for them to go stale, which is the freshness
  question [27](27-ldap-user-backend.md) raises for directory groups.
- If an immediate "log out everywhere" is ever needed: a per-user
  `token_version` held by the provider and checked on renewal. That is state
  on the user, not a session store. Not built until needed.

**Signing.** HS256 with the algorithm fixed on verification (the token's
`alg` header is never trusted), a `kid` so the key can be rotated without
logging everyone out, and the key resolved through the `SecretProvider`
chain. A key generated randomly at startup would work but log everyone out on
every restart, which is the one property of an in-memory session store this
was meant to avoid. Crate: `jsonwebtoken`.

**Other clients.** Scripts and CLIs against the REST server get API keys or
bearer tokens later. The same middleware then accepts either the cookie or an
`Authorization` header. The TUI runs in-process and needs no login page; how
it obtains an identity is the `--user` question in
[25](25-file-user-backend.md).

**Not solved by logging in.** Today there is one user (`--user`), and
`send_message` takes the first `UserActor` of the channel
(`ChannelLoop::user_actor()`). With several authenticated users, a message
must come from the `UserActor` of the user who sent it, and there must be a
rule for a user who is not a participant of the channel (`403`, or join).
Which sessions a user sees at all is the same question. That is
[09](09-creating-channels.md) and [11](11-agent-ownership.md). A login built
without it treats every authenticated user as the same channel participant.

## Done when

- The REST server rejects unauthenticated requests.
- `SecurityContext` at every entry point reflects an authenticated identity.
- A real `SecurityManager` replaces `DummySecurityManager` outside of tests.
- The web UI has a login page; the browser holds its identity as a JWT in an
  `HttpOnly` cookie with sliding renewal, a provider check on renewal and an
  absolute limit, as described above.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-21** — Browser sessions decided: login page, JWT in an `HttpOnly`,
  `SameSite=Strict` cookie instead of a bearer header (SSE cannot send
  headers, XSS) and instead of a server-side session store (multi-process).
  Non-revocability accepted and bounded by a short lifetime, sliding renewal
  with a provider check, and an absolute limit via `auth_time`. Noted that
  mapping the authenticated user onto the channel's `UserActor` is still open
  ([09](09-creating-channels.md), [11](11-agent-ownership.md)).
