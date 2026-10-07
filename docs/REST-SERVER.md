# REST Server

`caelumora-rest-server` makes the runtime available over HTTP: a REST API, two server-sent event
streams, and the login that guards both. It decides nothing itself - what a user may see or do is
decided by the runtime, and the server maps the answers to responses. This document describes how
the server is built, its endpoints, how it authenticates, how it answers errors, and its streams.
The browser frontend built on it is described in [WEB-UI.md](WEB-UI.md), the policy behind the
permissions in [SECURITY.md](SECURITY.md), and its configuration (`server`, `auth`) in
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

*   **It reaches a session through the runtime.** `ensure_session` opens the session for the
    authenticated user - the running one, or loaded from the store and started - and only if the
    user may read it (issue 11). It tells a session that does not exist (`404`) from one that cannot
    be started (`500`, naming e.g. the broken component).
*   **It acts on a channel through `open_channel`** (issue 102), the one way a handler that writes,
    approves, runs a command or joins the event stream gets the running channel. A handler that only
    reads - the history, the notes, what an agent saw - reaches the channel through the views
    (`routes::channel_of`, issue 163), running or not. Either way a channel the user may not read is
    answered as absent, like one that does not exist; the runtime decides that
    (`SessionLoop::channel_for`, `SessionView::channel`).
*   **It acts through the user's actor.** Writing, approving and running commands go through
    `user_actor_of`, which returns the authenticated user's own actor in the channel. Someone who is
    not a participant has none and is refused with `403` (issue 30). See
    [CHANNELS.md](CHANNELS.md#acting-in-a-channel).
*   **It reads through the views.** What a client lists and offers - sessions, channels, members,
    `can_change`, `can_read_context` - a handler reaches through the runtime's views as the user
    (`routes::views`, `SessionManager::views`, issue 163) and answers with their snapshots (issue
    80). `models.rs` maps them to responses (`From<snapshot::ChannelSnapshot> for ChannelResponse`
    and the like), and the server asks the `SecurityManager` nothing itself. See
    [SECURITY.md](SECURITY.md#views).

Every handler derives its `SecurityContext` from the `AuthenticatedUser` the authentication
middleware put into the request.

## Endpoints

Everything except `POST /auth/login` and `POST /auth/logout` requires a logged-in user.

| Endpoint | Does |
|---|---|
| `POST /auth/login`, `POST /auth/logout` | Log in and out |
| `GET /auth/me` | The logged-in user |
| `POST /auth/refresh` | Renew the session |
| `GET /templates` | The session templates a new session can be made from |
| `GET /compartments` | The configured compartments a channel's classification may name (issue 133) |
| `GET /profile`, `PUT /profile` | The logged-in user's own profile (issue 176): their `introduction`, `null` if none, `max_introduction_chars`, and the `preferred_language` their channels are held in, a BCP 47 tag or `null` (issue 56); `PUT` replaces both - `null` or empty removes the introduction, anything else is kept as written - and answers `400 invalid_value` for an introduction or a language that does not fit; `GET` also answers the user's `approval_rules` (issue 13), which `PUT` leaves as they are |
| `POST /profile/approval-rules`, `DELETE /profile/approval-rules/{id}` | Add one of the user's answers to tool calls given in advance (issue 13) - `{matcher, decision, reach?, expires_at?}`, `allow` or `deny`, everywhere unless a `{session}` or `{channel}` is named - answered `201` with the rule and its id; `400 invalid_rule` for one that asks, has a pattern that cannot be matched, or is one too many. Take one back: `204`, `404 rule_not_found` |
| `GET /sessions`, `POST /sessions` | List the user's sessions - with `?deleted=true` those they deleted and may read as such, the trash, each with `deleted` (who, when), `can_restore` and `can_purge` (issue 164); create one from a template (`SessionManager::create_from_template`) |
| `GET /agents`, `DELETE /agents/{id}` | The agents the user may read, each with `can_delete` - with `?deleted=true` the trash of agents, like that of sessions; delete one: it leaves every channel it takes part in (issue 164) |
| `POST /agents/{id}/restore`, `POST /agents/{id}/purge` | Restore a deleted agent - into no channel; `409 name_taken` if its name was taken meanwhile - or remove it for good with its state (issue 164) |
| `POST /sessions/{id}/restore`, `POST /sessions/{id}/purge` | Restore a deleted session - answered like `GET /sessions/{id}`, not started -, or remove it for good with its channels and their state; `403` unless the policy allows, by default the system's alone (issue 164) |
| `GET /sessions/{id}` | One session, in the form of the global stream's snapshot (`SessionSnapshot`), without starting it |
| `PATCH /sessions/{id}`, `DELETE /sessions/{id}` | Rename, delete a session - marked and kept, with its channels (issue 164) |
| `GET /sessions/{id}/actors/{aid}/private_notes` | The private notes an agent wrote in the channels of the session the user may read (issue 233): per channel its `channel_id`, `channel_name`, `notes` and the `digest` of their chain, sorted by the channel's name, channels without any left out - read as the agent's other private notes are, through `SessionView::private_notes_of`. `403 context_forbidden` for whoever may not read what the agent saw (`ReadAgentContext`) |
| `PUT /sessions/{id}/privacy` | Change whom the session lets read what was said in it - its owner (`privacy`: `private`, `same_owner` or `shared_audience`, issue 134); answers the `SessionSnapshot` |
| `PUT /sessions/{id}/description` | Say what the session is for - its owner (issue 245): `{summary, briefing}`, each replacing what it said, `null` or empty removing it, anything else kept as written; `400 invalid_value` for one that does not fit; answers the `SessionSnapshot`. Sessions carry `summary` - in lists too - and `briefing` where one is fetched, each left out where absent |
| `PUT /sessions/{id}/approval-rules` | Set what the session restricts of the tool calls in its channels - its owner (issue 13): a list of rules that `ask` or `deny`, replacing those it had; `400 invalid_rule` for one that allows, reaches less than the session or has a pattern that cannot be matched; answers the `SessionSnapshot`, which carries `approval_rules` |
| `GET …/channels`, `POST …/channels` | List a session's channels - with `?deleted=true` those deleted from it, like the sessions' trash (issue 164); create one, with an optional `classification` (`level` and `compartments`, see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)) and optional kinds of its `moderator` and `responder_policy` (see [CHANNELS.md](CHANNELS.md#moderators)), and an optional `language`, the user's preferred one if absent (issue 56); `409 name_taken` if the session has a channel of that name (issue 250) |
| `GET …/channels/{cid}`, `DELETE …/channels/{cid}` | One channel, with its `former_members`, each with `left` - who removed it and when (issue 164); delete it - marked and kept (issue 164) |
| `PUT …/channels/{cid}/classification` | Reclassify the channel - its owner (issue 133) |
| `PUT …/channels/{cid}/language` | Hold the channel in the language the body names - a BCP 47 tag such as `"de"` - or in none (`null`), whoever may change it (issue 56); answered like `GET` |
| `PUT …/channels/{cid}/description` | Say what the channel is for - whoever may change it (issue 245), as for the session; answered like `GET`. Channels carry `summary` always and `briefing` where one channel is fetched, in its stream's snapshot and in `GET` of its session - not in lists of channels |
| `PUT …/channels/{cid}/approval-rules` | Set what the channel restricts of the tool calls in it - whoever may change it (issue 13), as for the session; answered like `GET`, which carries `approval_rules` |
| `POST …/channels/{cid}/restore` | Restore a deleted channel into its session and start it again, answered like `GET`; `409 name_taken` if its name was taken meanwhile. A channel is purged only with its session (issue 164) |
| `PUT …/messages/{mid}/classification` | Reclassify a message - its author or the channel's owner, as a participant, through their actor (issues 133, 239; `message_not_found`, `409 message_revoked`, `403 not_a_participant`); shown on the channel's stream as `MessageReclassified` |
| `GET …/messages`, `POST …/messages` | The channel's history, older messages on request; write a message, with an optional `classification` that releases it (issue 133; refused as `invalid_classification`). Each message as it stands, with `modified` and `revoked` - when its author changed it - where set (issue 234) |
| `POST …/messages/{mid}/retry` | Ask the agent that said the message to answer again instead of it (issue 234, see [CHANNELS.md](CHANNELS.md#answering-again)), answered once the request is on its way; the agent's answer is on the stream - `RetryAccepted`, the message taken back and the new answer, or `RetryRejected` with a `reason`: `not_its_answer`, `not_last`, `calls_ran`, `asked_again`, `elsewhere` or `ended` (issue 235). The events name what they are about as `target`: `{"message": …}`, or `{"turn": …}` for an answer under way. `404 message_not_found`, `403 session_forbidden`, `409 message_revoked`, `409 not_an_agents_message`. Offered where the member's `can_retry` (issue 242) |
| `PUT …/messages/{mid}`, `DELETE …/messages/{mid}` | Modify one's own message - `{content, classification?}`, classified as it is if absent - or take it back, answered `204` (issue 234, see [CHANNELS.md](CHANNELS.md#changing-what-was-said)); `404 message_not_found`, `403 session_forbidden` for another's, `409 message_revoked` for one taken back already, `400 invalid_classification`. The stream shows the change as `MessageModified` or `MessageRevoked` |
| `POST …/approvals` | Approve or deny a tool call |
| `POST …/questions/{qid}` | Answer the questions of an agent's call, `{responses}` with one response per question - `{"kind": "text", "text"}`, `{"kind": "chosen", "chosen", "other"?}` or `{"kind": "declined"}` - or decline them, `{declined: true}` (issue 165, see [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#questions)). `404 question_not_found` if they are not open, `403 not_asked` if not put to the user, `400 invalid_answer` if the responses do not answer them as asked, `403 session_forbidden` |
| `POST …/cancel` | End the current turn of every agent in the channel |
| `POST …/participants/{aid}/retry` | Ask participant `aid`, an agent, to answer again instead of its answer under way, `{turn_id}` as `ResponseStarted` named it (issue 235, see [CHANNELS.md](CHANNELS.md#answering-again)); its turn is ended as the request is delivered, and the stream shows `RetryAccepted` and the new answer. `409 not_under_way` if the turn is not under way any more, `403 session_forbidden`, `404 participant_not_found`. Offered where the member's `can_retry` (issue 242) |
| `POST …/participants/{aid}/cancel` | End the current turn of one participant alone, under the same permission (issue 235, see [CHANNELS.md](CHANNELS.md#cancelling)); the channel does not become quiet. `403 cancel_forbidden`, `404 participant_not_found` |
| `GET …/notes` | The notes the channel's agents published |
| `POST …/participants`, `DELETE …/participants/{aid}` | Add a participant (with `history`), remove one; `not_cleared` if they are not cleared for the channel (issue 133) |
| `GET …/participants/{aid}` | One participant, with what its model calls used (`usage`, issue 209) - fetched again when an agent's turn ends, instead of the whole channel; a former member too, with `left` (issue 164). Also who it is (issue 244), each left out where absent: its `introduction` - an agent's by its personality, a user's own -, and of an agent the name of its `personality`, its `personality_kind` (`internal` or `external`), its `owner` and `owner_name`; public to whoever reads the channel, not given where members are listed |
| `GET …/actors/{aid}/messages/{mid}/context` | What an agent's model saw when it wrote a message |
| `GET …/actors/{aid}/history` | All that was offered to an agent and what it said itself, whole like the channel's messages - each with `has_context` and `in_transcript` (issue 207); like the context and the turns given up, also for a former member (issue 164) |
| `GET …/actors/{aid}/tools` | The tools an agent has now in the channel, in their tool sets - each tool with `requires_approval`, `visibility` and `classification` - as it would offer them at the start of its next turn (issue 194); `tool_sets` is empty where it has none here: no agent run here, a former member - and also a channel that does not run, whose agents' tools are not built. `403 agent_forbidden` for whoever may not see them (`ReadAgentTools`) |
| `GET …/actors/{aid}/given-up-turns` | The turns an agent gave up - failed or stopped - with their reason, the history message they branched off `after`, and their messages like the history's (issues 145, 146) |
| `GET …/commands`, `POST …/commands` | The commands the user may see; run one |
| `GET …/channels/{cid}/events` | The channel's stream |
| `GET /events` | The global stream |

`…` stands for `/sessions/{id}/channels/{cid}` (or `/sessions/{id}` for the list of channels). What
each operation does in the runtime is described with it: channels and participants in
[CHANNELS.md](CHANNELS.md), approvals and commands in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md), notes in [MEMORY.md](MEMORY.md), the context of a
message in [AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw). Renaming a channel
(`ChannelLoop::update_name`) has no endpoint yet.

When the REST API changes, `caelumora-web-ui` has to change with it: its types in `src/types` mirror
`models.rs`.

## Authentication

Login verifies username and password through the `UserManager` (see
[SECURITY.md](SECURITY.md#users)) and answers with a signed JWT (HS256) in a cookie.

*   **The cookie** is `HttpOnly`, `SameSite=Strict` and `Secure` - `Secure` unless
    `auth.secure_cookie` is switched off, for plain HTTP in a test setup.
*   **No server-side session store:** the token is the session.
*   **`require_auth`** (`auth.rs`) verifies the cookie on each request and puts an
    `AuthenticatedUser` into it, from which handlers derive their `SecurityContext`.

A cookie rather than an `Authorization` header, because `EventSource` cannot send headers, and an
`HttpOnly` cookie is out of reach of injected scripts.

### Renewal

Sessions are renewed by use: any request past half a token's lifetime gets fresh cookies.

Server-sent events cannot do that. One long response cannot set headers again, and their reconnects
deliberately do not renew either (see [Streams and the token](#streams-and-the-token)). So a client
that only watches has to renew by itself: the web UI calls `POST /auth/refresh` on a timer while its
tab is visible (issue 31, see [WEB-UI.md](WEB-UI.md)).

Next to the token cookie the server sets `caelumora_session_expires`, deliberately without
`HttpOnly`. It holds only the expiry, and lets the UI - and any other tab - see when the next
renewal is due.

### Bounding a token

A JWT cannot be revoked, so its validity is bounded instead:

*   Tokens are short-lived (`auth.token_lifetime_secs`, default 15 minutes) and renewed once past
    half their lifetime.
*   Only on renewal is the `UserManager` asked whether the user still exists.
*   The `auth_time` claim survives renewal, so a login ends after `auth.max_session_secs` (default
    12 hours) regardless.
*   The event streams end with the token they were opened with, so the bound holds for them too
    (issue 33).

The signing key is the secret named by `auth.jwt_secret`, resolved through the secret chain (see
[SECURITY.md](SECURITY.md#secrets)). If it is missing, a random key is generated at startup, and
every restart logs everybody out.

## Errors

Errors are answered as RFC 9457 problem details (`application/problem+json`, issue 02) with two
extension members:

*   `code`, a stable identifier, also the last segment of `type` (`urn:caelumora:problem:<code>`);
*   `request_id`, which also appears as the `x-request-id` header and on the request's log span.

Client errors state their cause. A text of the request that does not fit the field it is for - a
name, a language, a summary, an introduction, too long, of whitespace alone, of several lines where
one is asked for - is `400 invalid_value`, naming the field (`ValueError`, issue
[248](../issues/248-validated-text-fields.md)), wherever it is found out. A name that another of
its kind has in its name space - a channel's in its session, an agent's in its scope - is
`409 name_taken`, for every kind alike (issue [250](../issues/250-unique-names-per-name-space.md)).
Server errors state it only if `caelumora_runtime::error::user_message` allows it, and otherwise
point to the log. A refusal by the runtime (`caelumora_runtime::error::is_forbidden`) is a `403`,
not a `500`. How handlers produce these errors is a rule of
[CODING-GUIDELINES.md](CODING-GUIDELINES.md#rest-errors).

## Streams

The server has two server-sent event streams:

*   the **global stream**, `GET /events`, carrying `GlobalEvent`s from the event bus (see
    [EVENTS.md](EVENTS.md));
*   a **channel's stream**, `GET /sessions/{id}/channels/{cid}/events`, carrying `Envelope`s.

Both are built by one `sse::stream`: snapshot, resync on lag through `sse::resync_on_lag`, and the
end at the token's expiry or when the watch ends (issues 38, 104).

An `Envelope`'s event is an `ActorEvent` as serde writes it: an object named by the event, or a
bare string for the few without anything to carry (`"ResponseRequested"`, `"WakeupRequested"`). An
agent's answer begins with `{"ResponseStarted": {"turn_id": …}}`, and every message of it, partial
or complete, carries the same `turn_id` (issue 235) - also the messages of a channel's history, so
that a client tells which messages are one answer, and which answer is under way after a reload, by
the id rather than by who sent what when.

### Who follows a channel

A channel is followed live only by its participants, through their own actor (`UserActor::watch`,
issue 104). The stream is what their actor receives - what the moderator routes to the user - and
what it says for them (issue 220), and it ends when the actor ends: when the user left the channel,
or when the channel was stopped or deleted. Deleting a channel stops its actors, whether or not they
ran. Any number of streams may follow one actor - two tabs, two devices; one that falls more than
128 events behind loses the oldest and is sent a snapshot instead, if it asked for snapshots.

Anyone else who may read the channel - its session's owner, say - reads its history over REST but
does not follow it. Watching without taking part is issue 124, an invisible administrator issue 125.

### What the global stream shows

The global stream is filtered by what the user may read: its snapshot through the store (issue 11),
its events through `EventBus::subscribe_as` (issue 39).

Global events name sessions and channels by id alone. A client follows them with `GET
/sessions/{id}`, which answers in the snapshot's own form (`SessionSnapshot`, mapped from the same
`SessionView::snapshot`) without starting a stopped session.

### Snapshots

Both streams accept `?snapshot=true` and then begin with a named `snapshot` event describing the
state they start from (issue 34):

*   for a channel, its members and the last 20 messages, with `has_more`; older ones come over `GET
    .../messages`; the notes its agents published; and, for a user who takes part in it, the
    approvals its agents wait for (`pending_approvals`, issue 172), read from their stored turns -
    an approval asked for before the client connected, or before a restart, is shown too - and the
    questions they wait for the answers to (`pending_questions`, issue 165), sensitive ones only
    for those asked. Its
    members carry what their model calls used (`usage`, issue 209), as `GET …/participants/{aid}`
    does; where channels are listed - the global stream, `GET /sessions/{id}` - they do not, since
    it reads every agent's state (the `Detail` a handler asks the views for, see
    [SECURITY.md](SECURITY.md#views)). A member giving an answer now carries its turn
    (`turn_under_way`, issue 240), read from the running channel at no cost: a client that connects
    during an answer shows it as under way, as if it had seen its `ResponseStarted`. An external
    agent has no turn here and carries none. An agent member carries where it stands (`state`,
    issue 243) - `Idle`, `Sleeping`, `Planning`, `ExecutingTools`, `WaitingForUser`,
    `Reflecting` or `Cancelling` - read from the running channel alike and kept up by
    `{"StateChanged": …}` on the stream; absent in a channel that does not run. `can_cancel` says
    whether the viewer may end its answer (`POST …/participants/{aid}/cancel`), which ends one in
    `Planning`, `ExecutingTools` or `WaitingForUser`;
*   for the global stream, every session with its channels.

The order matters: the server subscribes *first* and reads the state afterwards, so nothing that
happens while the snapshot is assembled is lost. The price is that an event may arrive twice, once
in the snapshot and once as an event; clients drop duplicates by `MessageId` or entity id.

When a client falls behind and events are lost, a client that asked for snapshots is sent a fresh
one instead of a note that something is missing. This holds on both streams.

### Streams and the token

A stream does not outlive the token that authenticated it (issue 33). The authentication middleware
sees a stream once, when it connects, and never again. So the server ends the response itself, five
seconds before the token expires, announced by a named `expired` event. `EventSource` reconnects,
the new request is authenticated like any other, and the snapshot it asks for restores whatever
happened in the gap. A client that is no longer entitled gets a `401` there instead.

Every stream also begins with an SSE `retry` field of three seconds: the delay a client waits after
an *unexpected* break. A client that sees `expired` should reconnect at once, so that the planned
handover does not interrupt the output.

Streams authenticate through `require_stream_auth`, which differs from `require_auth` in one point:
it never renews the session. A stream that renewed would keep its own session alive for as long as
anything holds it open - a tab in the background, or a client that is not a browser at all - which
is precisely what the renewal rules are meant to bound. A stream therefore consumes a session; what
extends one is `POST /auth/refresh`.
