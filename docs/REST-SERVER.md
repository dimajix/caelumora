# REST Server

`caelumora-rest-server` makes the runtime available over HTTP: a REST API,
two server-sent event streams, and the login that guards both. It decides
nothing itself - what a user may see or do is decided by the runtime, and the
server maps the answers to responses. This document describes how the server
is built, its endpoints, how it authenticates, how it answers errors, and its
streams. The browser frontend built on it is described in
[WEB-UI.md](WEB-UI.md), the policy behind the permissions in
[SECURITY.md](SECURITY.md), and its configuration (`server`, `auth`) in
[CONFIGURATION.md](CONFIGURATION.md).

## Structure

| File | Holds |
|---|---|
| `main.rs` | Startup: configuration, stores, runtime, the router and its layers (request span, request id) |
| `state.rs` | `AppState`: the `Auth`, the `RuntimeManager` and the `SessionManager` every handler reaches |
| `auth.rs` | Login, logout, renewal, and the middleware `require_auth` and `require_stream_auth` |
| `routes/` | The handlers, by resource: `sessions`, `channels`, `templates`, `events` |
| `models.rs` | The request and response types, and their mapping from the runtime's views |
| `error.rs` | `ApiError` and the problem details it answers with; the `Json` and `Path` extractors |
| `sse.rs` | What both event streams have in common |

## How a handler works

A handler translates; it does not decide.

*   **It reaches a session through the runtime.** `ensure_session` opens the
    session for the authenticated user - the running one, or loaded from the
    store and started - and only if the user may read it (issue 11). It tells
    a session that does not exist (`404`) from one that cannot be started
    (`500`, naming e.g. the broken component).
*   **It reaches a channel through `open_channel`** (issue 102), the one way
    a handler gets a channel. A channel the user may not read is answered as
    absent, like one that does not exist; the runtime decides that
    (`SessionLoop::channel_for`).
*   **It acts through the user's actor.** Writing, approving and running
    commands go through `user_actor_of`, which returns the authenticated
    user's own actor in the channel. Someone who is not a participant has
    none and is refused with `403` (issue 30). See
    [CHANNELS.md](CHANNELS.md#acting-in-a-channel).
*   **It reads through the views.** What a client lists and offers -
    sessions, channels, members, `can_change`, `can_read_context` - a
    handler reaches through the runtime's views as the user
    (`routes::views`, `SessionManager::views`, issue 163) and answers
    with their snapshots (issue 80). `models.rs` maps them to responses
    (`From<snapshot::ChannelSnapshot> for ChannelResponse` and the like),
    and the server asks the `SecurityManager` nothing itself. See
    [SECURITY.md](SECURITY.md#views).

Every handler derives its `SecurityContext` from the `AuthenticatedUser` the
authentication middleware put into the request.

## Endpoints

Everything except `POST /auth/login` and `POST /auth/logout` requires a
logged-in user.

| Endpoint | Does |
|---|---|
| `POST /auth/login`, `POST /auth/logout` | Log in and out |
| `GET /auth/me` | The logged-in user |
| `POST /auth/refresh` | Renew the session |
| `GET /templates` | The session templates a new session can be made from |
| `GET /compartments` | The configured compartments a channel's classification may name (issue 133) |
| `GET /sessions`, `POST /sessions` | List the user's sessions; create one from a template (`SessionManager::create_from_template`) |
| `GET /sessions/{id}` | One session, in the form of the global stream's snapshot (`SessionSnapshot`), without starting it |
| `PATCH /sessions/{id}`, `DELETE /sessions/{id}` | Rename, delete a session |
| `PUT /sessions/{id}/privacy` | Change whom the session lets read what was said in it - its owner (`privacy`: `private`, `same_owner` or `shared_audience`, issue 134); answers the `SessionSnapshot` |
| `GET …/channels`, `POST …/channels` | List a session's channels; create one, with an optional `classification` (`level` and `compartments`, see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)) |
| `GET …/channels/{cid}`, `DELETE …/channels/{cid}` | One channel; delete it |
| `PUT …/channels/{cid}/classification` | Reclassify the channel - its owner (issue 133) |
| `PUT …/messages/{mid}/classification` | Reclassify a message - its author or the channel's owner (issue 133; `message_not_found`) |
| `GET …/messages`, `POST …/messages` | The channel's history, older messages on request; write a message, with an optional `classification` that releases it (issue 133; refused as `invalid_classification`) |
| `POST …/approvals` | Approve or deny a tool call |
| `POST …/cancel` | End the current turn of every agent in the channel |
| `GET …/notes` | The notes the channel's agents published |
| `POST …/participants`, `DELETE …/participants/{aid}` | Add a participant (with `history`), remove one; `not_cleared` if they are not cleared for the channel (issue 133) |
| `GET …/actors/{aid}/messages/{mid}/context` | What an agent's model saw when it wrote a message |
| `GET …/actors/{aid}/given-up-turns` | The turns an agent gave up - failed or stopped - with their reason and messages (issue 145) |
| `GET …/commands`, `POST …/commands` | The commands the user may see; run one |
| `GET …/channels/{cid}/events` | The channel's stream |
| `GET /events` | The global stream |

`…` stands for `/sessions/{id}/channels/{cid}` (or `/sessions/{id}` for the
list of channels). What each operation does in the runtime is described with
it: channels and participants in [CHANNELS.md](CHANNELS.md), approvals and
commands in [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md), notes in
[MEMORY.md](MEMORY.md), the context of a message in
[AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw). Renaming a channel
(`ChannelLoop::update_name`) has no endpoint yet.

When the REST API changes, `caelumora-web-ui` has to change with it: its
types in `src/types` mirror `models.rs`.

## Authentication

Login verifies username and password through the `UserManager` (see
[SECURITY.md](SECURITY.md#users)) and answers with a signed JWT (HS256) in a
cookie.

*   **The cookie** is `HttpOnly`, `SameSite=Strict` and `Secure` - `Secure`
    unless `auth.secure_cookie` is switched off, for plain HTTP in a test
    setup.
*   **No server-side session store:** the token is the session.
*   **`require_auth`** (`auth.rs`) verifies the cookie on each request and
    puts an `AuthenticatedUser` into it, from which handlers derive their
    `SecurityContext`.

A cookie rather than an `Authorization` header, because `EventSource` cannot
send headers, and an `HttpOnly` cookie is out of reach of injected scripts.

### Renewal

Sessions are renewed by use: any request past half a token's lifetime gets
fresh cookies.

Server-sent events cannot do that. One long response cannot set headers
again, and their reconnects deliberately do not renew either (see
[Streams and the token](#streams-and-the-token)). So a client that only
watches has to renew by itself: the web UI calls `POST /auth/refresh` on a
timer while its tab is visible (issue 31, see [WEB-UI.md](WEB-UI.md)).

Next to the token cookie the server sets `caelumora_session_expires`,
deliberately without `HttpOnly`. It holds only the expiry, and lets the UI -
and any other tab - see when the next renewal is due.

### Bounding a token

A JWT cannot be revoked, so its validity is bounded instead:

*   Tokens are short-lived (`auth.token_lifetime_secs`, default 15 minutes)
    and renewed once past half their lifetime.
*   Only on renewal is the `UserManager` asked whether the user still exists.
*   The `auth_time` claim survives renewal, so a login ends after
    `auth.max_session_secs` (default 12 hours) regardless.
*   The event streams end with the token they were opened with, so the bound
    holds for them too (issue 33).

The signing key is the secret named by `auth.jwt_secret`, resolved through
the secret chain (see [SECURITY.md](SECURITY.md#secrets)). If it is missing,
a random key is generated at startup, and every restart logs everybody out.

## Errors

Errors are answered as RFC 9457 problem details (`application/problem+json`,
issue 02) with two extension members:

*   `code`, a stable identifier, also the last segment of `type`
    (`urn:caelumora:problem:<code>`);
*   `request_id`, which also appears as the `x-request-id` header and on the
    request's log span.

Client errors state their cause. Server errors state it only if
`caelumora_runtime::error::user_message` allows it, and otherwise point to
the log. A refusal by the runtime (`caelumora_runtime::error::is_forbidden`)
is a `403`, not a `500`. How handlers produce these errors is a rule of
[CODING-GUIDELINES.md](CODING-GUIDELINES.md#rest-errors).

## Streams

The server has two server-sent event streams:

*   the **global stream**, `GET /events`, carrying `GlobalEvent`s from the
    event bus (see [EVENTS.md](EVENTS.md));
*   a **channel's stream**, `GET /sessions/{id}/channels/{cid}/events`,
    carrying `Envelope`s.

Both are built by one `sse::stream`: snapshot, resync on lag through
`sse::resync_on_lag`, and the end at the token's expiry or when the watch
ends (issues 38, 104).

### Who follows a channel

A channel is followed live only by its participants, through their own actor
(`UserActor::watch`, issue 104). The stream is every event the channel
publishes, and it ends when the actor ends: when the user left the channel,
or when the channel was stopped or deleted. Deleting a channel stops its
actors, whether or not they ran.

Anyone else who may read the channel - its session's owner, say - reads its
history over REST but does not follow it. Watching without taking part is
issue 124, an invisible administrator issue 125.

### What the global stream shows

The global stream is filtered by what the user may read: its snapshot through
the store (issue 11), its events through `EventBus::subscribe_as` (issue 39).

Global events name sessions and channels by id alone. A client follows them
with `GET /sessions/{id}`, which answers in the snapshot's own form
(`SessionSnapshot`, mapped from the same `SessionView::snapshot`) without starting a
stopped session.

### Snapshots

Both streams accept `?snapshot=true` and then begin with a named `snapshot`
event describing the state they start from (issue 34):

*   for a channel, its members and the last 20 messages, with `has_more`;
    older ones come over `GET .../messages`;
*   for the global stream, every session with its channels.

The order matters: the server subscribes *first* and reads the state
afterwards, so nothing that happens while the snapshot is assembled is lost.
The price is that an event may arrive twice, once in the snapshot and once as
an event; clients drop duplicates by `MessageId` or entity id.

When a client falls behind and events are lost, a client that asked for
snapshots is sent a fresh one instead of a note that something is missing.
This holds on both streams.

### Streams and the token

A stream does not outlive the token that authenticated it (issue 33). The
authentication middleware sees a stream once, when it connects, and never
again. So the server ends the response itself, five seconds before the token
expires, announced by a named `expired` event. `EventSource` reconnects, the
new request is authenticated like any other, and the snapshot it asks for
restores whatever happened in the gap. A client that is no longer entitled
gets a `401` there instead.

Every stream also begins with an SSE `retry` field of three seconds: the
delay a client waits after an *unexpected* break. A client that sees
`expired` should reconnect at once, so that the planned handover does not
interrupt the output.

Streams authenticate through `require_stream_auth`, which differs from
`require_auth` in one point: it never renews the session. A stream that
renewed would keep its own session alive for as long as anything holds it
open - a tab in the background, or a client that is not a browser at all -
which is precisely what the renewal rules are meant to bound. A stream
therefore consumes a session; what extends one is `POST /auth/refresh`.
