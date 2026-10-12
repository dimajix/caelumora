# REST Server

`caelumora-rest-server` makes the runtime available over HTTP: a REST API, two server-sent event
streams, and the login that guards both. It is a library; the program that serves it is
`caelumora-app`, under `/api`, with the web UI beside it. It decides nothing itself - what a user
may see or do is decided by the runtime, and the server maps the answers to responses. The browser
frontend built on it is described in [WEB-UI.md](WEB-UI.md), the policy behind the permissions in
[SECURITY.md](SECURITY.md), and its configuration (`server`, `auth`) in
[CONFIGURATION.md](CONFIGURATION.md).

## Structure

| File | Holds |
|---|---|
| `state.rs` | `AppState`: the `Auth`, the `RuntimeManager` and the `SessionManager` every handler reaches |
| `auth.rs` | Login, logout, renewal, and the middleware `require_auth` and `require_stream_auth` |
| `routes/` | The handlers, by resource, and `routes::api`, the router of all of them and of the plugins' routes |
| `plugin.rs` | `RestPlugin`, a plugin that serves routes of its own |
| `error.rs` | `ApiError` and the problem details it answers with; the `Json` and `Path` extractors |
| `sse.rs` | What both event streams have in common |

The request and response types are not the server's: they lie in the crate `caelumora-rest-api`,
one module per subject (`session`, `channel`, `workspace`, …), which depends on the model alone, so
that a client reads them without the server (issue 323). A response is made there from the model's
entities and snapshots, by `From` or an `of`. What only the runtime knows - the names of the places
a message's knowledge was read in, a file of a workspace, a command of its shell - the handler puts
into the response itself.

### The program

`caelumora-app` is the composition root: its `main.rs` reads the configuration, lists the plugins
(`plugins()`), builds the runtime with them (`RuntimeBuilder`), makes the `AppState`, and serves:

*   the API under `/api` - `routes::api` nested there, with the routes of the plugins, every path of
    this document below it, and a path no route takes answered as a problem (`404 not_found`) like
    every other error;
*   the web UI everywhere else, with the feature `web-ui` (on by default): the files `npm run build`
    made in `caelumora-web-ui/dist`, embedded in a release build and read from there in a debug
    build. A path that names no file is answered with `index.html`, which is never cached; the files
    under `assets/`, whose names change with their content, are cached for long. Without the feature
    the program serves the API alone.

The build does not build the UI: `build.rs` checks that `dist/index.html` is there and otherwise
fails, saying how to build it; with `CAELUMORA_BUILD_UI=1` it runs `npm ci` and `npm run build`
itself. Node is a requirement of a build with the UI built in, not of every `cargo build`.

The request span and the request id (`error::request_id`) are layers the program puts around
everything it serves.

### The routes of a plugin

A plugin that serves routes of its own (issue 323) implements `RestPlugin`, a `Plugin` with
`routes(state)`: a `Router` built with the `AppState`, from which the plugin takes what its handlers
need - its manager among the runtime's services - into a state of their own, so that a handler does
not reach it through the runtime at every request. `routes::api` merges the routes of every plugin
the program lists behind the same login as the core's (`require_auth`). A handler builds on the
public modules of this library - `auth`, `error`, `state`, `sse` -, answers its errors as the core's
do (a plugin implements `From<…> for ApiError` for its own error types, see [Errors](#errors)), and
its paths are the plugin's to choose, as long as no core route has them.

## How a handler works

A handler translates; it does not decide. Every handler derives its `SecurityContext` from the
`AuthenticatedUser` the authentication middleware put into the request.

*   **It reaches a session through the runtime.** `ensure_session` opens the session for the user -
    the running one, or loaded and started - and only if the user may read it. It tells a session
    that does not exist (`404`) from one that cannot be started (`500`, naming e.g. the broken
    component).
*   **It acts on a channel through `open_channel`**, the one way a handler that writes, approves,
    runs a command or joins the event stream gets the running channel. A handler that only reads -
    the history, the notes, what an agent saw - reaches the channel through the views
    (`routes::channel_of`), running or not. Either way a channel the user may not read is answered
    as absent, like one that does not exist; the runtime decides that.
*   **It acts through the user's actor.** Writing, approving and running commands go through
    `user_actor_of`, the user's own actor in the channel. Someone who is not a participant has none
    and is refused with `403`. See [CHANNELS.md](CHANNELS.md#acting-in-a-channel).
*   **It reads through the views.** What a client lists and offers - sessions, channels, members,
    `can_change`, `can_read_context` - a handler reaches through the runtime's views as the user and
    answers with their snapshots, which `caelumora-rest-api` maps to responses. The server asks the
    `SecurityManager` nothing itself. See [SECURITY.md](SECURITY.md#views).

## Endpoints

Every path below lies under `/api`. Everything except `POST /auth/login` and `POST /auth/logout`
requires a logged-in user. `…` stands
for `/sessions/{id}/channels/{cid}` (or `/sessions/{id}` for the list of channels). What each
operation does in the runtime is described with it - channels and participants in
[CHANNELS.md](CHANNELS.md), approvals and questions in [APPROVALS.md](APPROVALS.md), commands in
[COMMANDS.md](COMMANDS.md), notes in [MEMORY.md](MEMORY.md), the context of a message in
[AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw) - and the request and response of each in
its handler's doc comment.

| Endpoint | Does |
|---|---|
| `POST /auth/login`, `POST /auth/logout`, `GET /auth/me`, `POST /auth/refresh` | Log in and out, the logged-in user, renew the session |
| `GET /templates`, `GET /compartments` | The session templates, with the workspace their sessions get; the compartments a classification may name |
| `GET`, `PUT /profile`; `POST /profile/approval-rules`, `DELETE /profile/approval-rules/{id}` | The user's own profile - introduction, preferred language, approval rules; add or take back one of the rules |
| `GET`, `POST /sessions`; `GET`, `PATCH`, `DELETE /sessions/{id}`; `POST /sessions/{id}/restore`, `…/purge` | Sessions: list, create from a template (with a workspace chosen instead of the template's), fetch without starting, rename, delete, restore, purge |
| `PUT /sessions/{id}/privacy`, `…/description`, `…/approval-rules`; `PUT`, `DELETE /sessions/{id}/workspace` | Change whom the session lets read what was said, what it is for, what it restricts of tool calls; attach or take away its workspace |
| `GET /sessions/{id}/actors/{aid}/private_notes` | The private notes an agent wrote in the channels of the session the user may read |
| `GET /agents`, `DELETE /agents/{id}`, `POST /agents/{id}/restore`, `…/purge` | Agents: list, delete (it rests in its channels as a former member), restore (back into them), purge |
| `GET`, `POST /workspaces`; `GET /workspaces/storages`; `GET`, `PATCH`, `DELETE /workspaces/{id}`; `PUT /workspaces/{id}/access`, `…/description`, `…/classification`; `POST /workspaces/{id}/restore`, `…/purge` | Workspaces: list, make, the storages they can be made in, fetch, rename, delete, share, describe, reclassify, restore, purge |
| `GET /workspaces/{id}/files?path=`; `GET`, `PUT`, `DELETE /workspaces/{id}/files/{path}` | List a directory of a workspace, download, upload or delete a file |
| `GET`, `POST /skills`; `GET`, `PATCH`, `DELETE /skills/{id}`; `PUT /skills/{id}/description`, `…/body`, `…/classification`; `POST /skills/{id}/restore`, `…/purge` | Skills: list without bodies, make, fetch with body, rename, delete, change, restore, purge |
| `GET`, `POST /groups`; `GET`, `PATCH`, `DELETE /groups/{id}`; `PUT /groups/{id}/members`; `POST /groups/{id}/restore`, `…/purge` | Teams, and the groups of the participants of a session or channel the user reads: list, make, fetch, rename, delete, set a team's members, restore, purge |
| `GET /artifacts`; `GET`, `DELETE /artifacts/{id}`; `PUT /artifacts/{id}/scope`, `…/description`, `…/access`; `POST /artifacts/{id}/restore`, `…/purge` | Artifacts: list the user's own and those shared with them, fetch, delete, keep beyond the session, describe, share, restore, purge |
| `GET /artifacts/{id}/versions/{n}/content` | Download a version's content |
| `GET`, `POST …/channels`; `GET`, `DELETE …/channels/{cid}`; `POST …/channels/{cid}/restore` | Channels: list, create, fetch (with its former members), delete, restore |
| `PUT …/channels/{cid}/classification`, `…/language`, `…/description`, `…/uses-workspace`, `…/approval-rules` | Change a channel: reclassify it, its language, what it is for, whether it uses the session's workspace, what it restricts of tool calls |
| `GET …/channels/{cid}/artifacts` | The artifacts with a version published in the channel |
| `POST …/channels/{cid}/commands/{command_id}/kill` | End a command of the workspace's shell started in the channel, with what it started |
| `GET …/history`, `POST …/messages` | The channel's history as items; write a message |
| `PUT`, `DELETE …/messages/{mid}`; `PUT …/messages/{mid}/classification`; `POST …/messages/{mid}/retry` | Modify or take back one's own message; reclassify a message; ask the agent that said it to answer again |
| `POST …/approvals`, `POST …/questions/{qid}` | Approve or deny a tool call; answer or decline the questions of an agent's call |
| `POST …/cancel` | End the current turn of every agent in the channel |
| `POST`, `DELETE …/participants`, `…/participants/{aid}`; `GET …/participants/{aid}` | Add or remove a participant; one participant, with what its model calls used and who it is |
| `POST …/participants/{aid}/retry`, `…/cancel`, `…/tool-calls/{call_id}/cancel` | Ask one agent to answer again; end its turn alone; interrupt one tool call of its turn |
| `GET …/notes` | The notes the channel's agents published |
| `GET …/actors/{aid}/messages/{mid}/context` | What an agent's model saw when it wrote a message: `403 context_forbidden`, `404 context_not_recorded` (not produced by that actor), `404 context_unavailable` (recorded, no longer resolvable) |
| `GET …/actors/{aid}/history`, `…/components`, `…/tools` | All that was offered to an agent and what it said; the components it was built from in the channel; the tools it has now |
| `GET`, `POST …/commands` | The commands of the channel the user may see; issue one |
| `GET …/channels/{cid}/events`, `GET /events` | The channel's stream; the global stream |

Some rules hold across the endpoints:

*   **Deleted things are kept.** `DELETE` marks and keeps; `?deleted=true` on a list is the trash,
    each entry saying whether the user may restore or purge it. Restoring answers `409 name_taken`
    if the name was taken meanwhile. Deleting a session or channel answers `409 scope_not_empty`
    if something of its scope is left that the user may not delete; what they may is deleted.
    Purging is as the policy allows, by default the system's alone. A channel is purged only with
    its session.
*   **Files are reached by their workspace alone**, never through a channel; whoever the workspace's
    access list lets read and who is cleared for its classification reads them, whoever it lets
    change them writes (`403 read_only` otherwise). A download - of a file or of an artifact's
    version - is always `application/octet-stream`, `Content-Disposition: attachment`, `nosniff`
    and `no-store`: never shown inline.
*   **Sending into a channel is answered once it is on its way** - a message, a change to one, a
    retry, the answers to questions. Whether the channel lets it in arrives on the stream (see [Who
    follows a channel](#who-follows-a-channel)).
*   **What costs, or concerns one thing, is given where one thing is fetched**, not where things are
    listed: what a member's model calls used, a briefing, an artifact's places of publication. An
    access list is given to the owner alone.
*   **What a client offers it reads from flags** (`can_change`, `can_retry`, `can_cancel`,
    `can_read_context`, ...) the views decide for the user, not from rules of its own.

Renaming a channel (`ChannelLoop::update_name`) has no endpoint yet.

When the REST API changes, `caelumora-web-ui` has to change with it: its types in `src/types` mirror
those of `caelumora-rest-api`.

## Authentication

Login verifies username and password through the `UserManager` (see
[SECURITY.md](SECURITY.md#users)) and answers with a signed JWT (HS256) in a cookie.

*   **The cookie** is `HttpOnly`, `SameSite=Strict` and `Secure` - `Secure` unless
    `auth.secure_cookie` is switched off, for plain HTTP in a test setup.
*   **No server-side session store:** the token is the session.
*   **`require_auth`** verifies the cookie on each request and puts an `AuthenticatedUser` into it.

A cookie rather than an `Authorization` header, because `EventSource` cannot send headers, and an
`HttpOnly` cookie is out of reach of injected scripts.

### Renewal

Sessions are renewed by use: any request past half a token's lifetime gets fresh cookies. A renewal
asks the `UserManager` about the user, so roles given or taken meanwhile take effect then (see
[SECURITY.md](SECURITY.md#users)).

Server-sent events cannot do that: one long response cannot set headers again, and their reconnects
deliberately do not renew (see [Streams and the token](#streams-and-the-token)). So a client that
only watches renews by itself: the web UI calls `POST /auth/refresh` on a timer while its tab is
visible (see [WEB-UI.md](WEB-UI.md)).

Next to the token cookie the server sets `caelumora_session_expires`, deliberately without
`HttpOnly`. It holds only the expiry, and lets the UI - and any other tab - see when the next
renewal is due.

### Bounding a token

A JWT cannot be revoked, so its validity is bounded instead:

*   Tokens are short-lived (`auth.token_lifetime_secs`) and renewed once past half their lifetime.
*   Only on renewal is the `UserManager` asked whether the user still exists.
*   The `auth_time` claim survives renewal, so a login ends after `auth.max_session_secs`
    regardless.
*   The event streams end with the token they were opened with, so the bound holds for them too.

The signing key is the secret named by `auth.jwt_secret`, resolved through the secret chain (see
[SECURITY.md](SECURITY.md#secrets)). If it is missing, a random key is generated at startup, and
every restart logs everybody out.

## Errors

Errors are answered as RFC 9457 problem details (`application/problem+json`) with two extension
members:

*   `code`, a stable identifier, also the last segment of `type` (`urn:caelumora:problem:<code>`);
    a client decides by it rather than by the text;
*   `request_id`, which also appears as the `x-request-id` header and on the request's log span.

Client errors state their cause. A text of the request that does not fit the field it is for - too
long, of whitespace alone, of several lines where one is asked for - is `400 invalid_value`, naming
the field (`ValueError`), wherever it is found out. A name that another of its kind has in its name
space - a channel's in its session, an agent's in its scope - is `409 name_taken`, for every kind
alike. Something the user may not read is answered as absent (`404`), like something that does not
exist.

Every error answers itself, by its type and its variant, not by what the handler was doing (issue
323): each error type that reaches a handler implements `From<…> for ApiError` in `error.rs` - or,
for a plugin's, in the plugin - and a handler passes it on with `?`.

*   **A refusal** is `403 <subject>_forbidden`, named by what was refused: the error carries the
    runtime's `Refused` as its source, and its subject is the kind of the entity (`skill_forbidden`,
    `session_forbidden`) or what one of the core's own actions concerns (`context_forbidden` for
    what an agent saw, `agent_forbidden` for its tools and components, `command_forbidden`, …). The
    detail is the runtime's text. A refusal is found anywhere in an error's chain, also in an error
    of a type the server does not know.
*   **What a request named and was not found** is told by the operation's own error
    (`SessionError::NotFound`, `ViewError::ChannelNotFound`, …), as `<kind>_not_found`. A
    `StoreError::NotFound` met on the way - a personality missing while a session is loaded - is a
    server error.
*   **An `anyhow::Error`** is answered by an invalid value anywhere in its chain, then by its
    outermost error if that is of a type the core knows, then by a name taken or a refusal anywhere
    in the chain.
*   **A server error** is `500 internal_error` (or a code of the error type's own, such as
    `command_failed`), and states its cause only if `caelumora_runtime::error::user_message` allows
    it, otherwise pointing to the log; the full chain is logged in the request's span.

How handlers produce errors is a rule of [CODING-GUIDELINES.md](CODING-GUIDELINES.md#rest-errors).

## Streams

The server has two server-sent event streams:

*   the **global stream**, `GET /events`, carrying `GlobalEvent`s from the event bus (see
    [EVENTS.md](EVENTS.md));
*   a **channel's stream**, `GET /sessions/{id}/channels/{cid}/events`, carrying `Envelope`s.

Both are built by one `sse::stream`: snapshot, resync on lag, and the end at the token's expiry or
when the watch ends.

An `Envelope`'s event is an `ActorEvent` as serde writes it. An agent's answer begins with
`ResponseStarted` and its `turn_id`, and every message of it, partial or complete, carries the same
`turn_id` - also in a channel's history - so that a client tells which messages are one answer, and
which answer is under way after a reload, by the id rather than by who sent what when. A user's
message carries none: it is said outside of any turn. A turn ends with `TurnFinished` or
`TurnGivenUp` and its reason; a stopped agent sends neither.

A history - the channel's and an agent's - is a list of items in the order they happened: messages
said, where a step of a plan began, how turns ended, commands an agent carried out, and - in an
agent's history only - compactions of its context. Read from another channel, a history holds only
the messages released, and neither a turn's end nor the start of a step.

### Who follows a channel

A channel is followed live only by its participants, through their own actor (`UserActor::watch`).
The stream is what their actor receives - what the moderator routes to the user - what it says for
them, and why the channel did not let in what the user sent (`MessageRejected` with its reason). A
request that sends something into the channel is answered once it is on its way, so a refusal
arrives on the stream, after what the user sent. The stream ends when the actor ends: when the user
left the channel, or when the channel was stopped or deleted. Any number of streams may follow one
actor - two tabs, two devices; one that falls too far behind loses the oldest events and is sent a
snapshot instead, if it asked for snapshots.

Anyone else who may read the channel - its session's owner, say - reads its history over REST but
does not follow it.

### What the global stream shows

The global stream is filtered by what the user may read: its snapshot through the store, its events
through `EventBus::subscribe_as`. An event is written as serde writes a `GlobalEvent` (see
[EVENTS.md](EVENTS.md#the-form-of-an-event)): one about an entity as `{"Entity": {"kind": "session",
"id": "…", "change": "Deleted"}}`, with `{"Updated": …}` or `{"Custom": {"name": "started", "data":
null}}` as its change; one of a channel or its members as `{"ChannelDeleted": ["<session id>",
"<channel id>"]}`. Global events name sessions and channels by id alone; a client follows them
with `GET /sessions/{id}`, which answers in the snapshot's own form without starting a stopped
session.

### Snapshots

Both streams accept `?snapshot=true` and then begin with a named `snapshot` event describing the
state they start from:

*   for a channel, its members, the last messages with `has_more` (older ones over REST) and the
    notes its agents published; for a participant, also the approvals and questions its agents wait
    for, read from their stored turns, so that one asked before the client connected or before a
    restart is shown - a sensitive question only to whom it was asked. A member giving an answer
    carries its turn (`turn_under_way`) and an agent member where it stands (`state`), both read
    from the running channel and kept up by the stream; a client that connects during an answer
    shows it as under way. Only here, not where channels are listed, do members carry what their
    model calls used, since that reads every agent's state;
*   for the global stream, every session with its channels.

The order matters: the server subscribes *first* and reads the state afterwards, so nothing that
happens while the snapshot is assembled is lost. The price is that an event may arrive twice;
clients drop duplicates by `MessageId` or entity id.

When a client falls behind and events are lost, a client that asked for snapshots is sent a fresh
one instead of a note that something is missing. This holds on both streams.

### Streams and the token

A stream does not outlive the token that authenticated it. The authentication middleware sees a
stream once, when it connects, and never again. So the server ends the response itself shortly
before the token expires, announced by a named `expired` event. `EventSource` reconnects, the new
request is authenticated like any other, and the snapshot it asks for restores whatever happened in
the gap. A client that is no longer entitled gets a `401` there instead.

Every stream also begins with an SSE `retry` field: the delay a client waits after an *unexpected*
break. A client that sees `expired` should reconnect at once, so that the planned handover does not
interrupt the output.

Streams authenticate through `require_stream_auth`, which differs from `require_auth` in one point:
it never renews the session. A stream that renewed would keep its own session alive for as long as
anything holds it open - a tab in the background, or a client that is not a browser at all - which
is precisely what the renewal rules are meant to bound. A stream therefore consumes a session; what
extends one is `POST /auth/refresh`.
