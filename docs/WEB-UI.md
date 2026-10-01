# Web UI

`caelumora-web-ui` is the browser frontend: React and TypeScript, built with
Vite and styled with Tailwind. It talks to the REST server only, over the REST
API and its two event streams. This document describes how it is built and
run, how it talks to the server, and what it shows. The server side of every
request is described in [REST-SERVER.md](REST-SERVER.md).

## Running it

```bash
cd caelumora-web-ui
npm install
npm run dev
```

The development server proxies `/api` to the REST server on
`http://localhost:3000`, stripping the prefix (`vite.config.ts`); the REST
server is started as described in the [README](../README.md).
`npm run build` type-checks and builds into `dist/`, `npm run lint` runs
ESLint.

## What the UI decides, and what it does not

The UI shows what the server says and repeats no rule of its own:

*   **Permissions come from the server.** Whether the user may change a
    channel (`can_change`) or see what an agent saw (`can_read_context`) is
    part of the responses, decided by the runtime's views (see
    [SECURITY.md](SECURITY.md#views)). The UI offers an action only where it
    is set, and never works it out itself.
*   **Only lexical rules are mirrored.** Whether a line is a command
    (`command.ts`, mirroring `caelumora_model::command::parse_input`) and how
    a participant's handle is derived (`mention.ts`, mirroring
    `caelumora_model::mention`) are repeated here, because the UI needs them
    while the user types. What a command means and whom a mention addresses
    is decided by the server.
*   **The types mirror the server.** `src/types` holds the request and
    response types of `caelumora-rest-server/src/models.rs`; a change to the
    REST API changes them too.

## Structure

| File | Holds |
|---|---|
| `src/api/index.ts` | Every REST call, relative to `/api`, with the session cookie |
| `src/types/index.ts` | The request and response types |
| `src/errors.ts` | Problem details as `ApiRequestError`, and how errors are reported |
| `src/session.ts` | Keeping the session alive; drafts and the last template in browser storage |
| `src/stream.ts` | `openEventStream`, for both event streams |
| `src/globalEvents.ts` | What the session list does with a global event |
| `src/command.ts`, `src/mention.ts` | The lexical rules mirrored from the model |
| `src/messages.ts` | Helpers for showing messages, shared by the chat and the context panel |
| `src/moderator.ts`, `src/confidentiality.ts` | Labels and explanations of moderators and classifications |
| `src/App.tsx` | Login state, keep-alive, the error banner, the layout |
| `src/components/` | `Login`, `Sidebar`, `ChatView`, `MessageView`, `MarkdownMessage`, `ContextPanel`, `NotesList`, `NewChannelDialog`, `AddParticipantDialog`, `PrivacyDialog`, `ErrorBanner` |

## Talking to the server

### Login and keeping the session

The session is an `HttpOnly` cookie the UI never sees; `api` sends it with
every request. A `401` brings up the login page.

A user who only reads would be logged out mid-conversation, because the
event streams cannot renew the token (see
[REST-SERVER.md](REST-SERVER.md#renewal)). So `session.ts` renews by itself
with `POST /auth/refresh` while its tab is visible, and only then - a tab in
the background is not someone watching (issue 31). When to renew is read from
the `caelumora_session_expires` cookie, which also tells a tab about a
renewal another tab made.

### Errors

The server answers errors with problem details (see
[REST-SERVER.md](REST-SERVER.md#errors)). `ApiRequestError` carries their
`detail` as message and the `request_id`, under which the full error is in
the server log. Failed operations are listed in the error banner, newest
last, each with its request id; dialogs show what the server says about a
request they can correct - a name that is taken, say - next to the field.

### The event streams

Both streams go through one `openEventStream` (`src/stream.ts`), so the
snapshot, the reconnect and the check of the session are handled in one
place:

*   It always asks for a snapshot (`?snapshot=true`), and gets one again
    after every reconnect.
*   On `expired` it reconnects at once. The new request carries the renewed
    cookie, and its snapshot restores what happened in between, so the
    handover is invisible.
*   An unexpected break is left to the browser's own retry. A stream that is
    really closed checks the session (`GET /auth/me`), whose `401` brings up
    the login page if that was the cause.

Messages can arrive twice, in the snapshot and as an event; they are dropped
by id.

## What it shows

### The session list

The sidebar lists the user's sessions with their channels. It follows the
global stream (issue 35, `globalEvents.ts`): it takes its state from the
snapshot, removes a session on `SessionDeleted`, takes a new name from
`SessionUpdated`, and fetches a session again (`GET /sessions/{id}`) on any
other `SessionUpdated` - a changed privacy - and on any event that changes
its channels or their members. Starting and stopping are ignored.

A session's privacy (issue 134, `privacy.ts`) is shown to everyone in it:
a session other sessions may read is marked in the list, and its menu opens
`PrivacyDialog`, where its owner (`can_change`) changes it. Choosing a
policy that lets other sessions read says that it holds for what is said
from then on.

A new session is created from a template; the one used last in this browser
is preselected (issue 40). A new channel is created with its name, moderator,
classification - a level and, if any are configured, compartments from
`GET /compartments` (issue 133) - and participants (`NewChannelDialog`,
issue 09).

### A channel

`ChatView` shows one channel. Only a participant follows it live, over the
channel's stream; anyone else who may read it - the session's owner, say - is
shown its history and no input.

*   **History.** The snapshot holds the last 20 messages; older ones are
    fetched on request. A message is shown with its tool calls, their status,
    their result and how long they took (`MessageView`); a call that is not
    public is marked with how far it may travel (issues 54, 61, 100).
*   **While an agent answers**, its partial message is shown as it streams.
    A failed turn and the channel-wide answer of a command are shown in the
    chat but never stored. When a participant is removed, the channel says so
    with `ParticipantRemoved`, and nothing of it follows: its partial answer
    and the stop button go, and its calls still without a result are shown as
    abandoned (issue 162).
*   **Approvals.** A tool call that needs approval is shown with approve and
    deny. Once this user answered, it waits for the decision; it disappears
    only when the approver ends it - decided, timed out, or withdrawn by an
    agent whose turn ended (issue 148). Pending approvals are known only
    from the stream: a reload loses them
    ([172](../issues/172-pending-approvals-not-kept.md)).
*   **Stop.** The stop button ends the current turn of every agent
    (`POST …/cancel`).
*   **Input.** A line starting with `/` is sent as a command, `//` as a
    literal slash (`command.ts`). Typing `@` completes the members' handles,
    `/` the commands the user may see. An unsent draft survives a forced
    re-login, in the session storage.

### Classification

The channel's classification is shown above the member list. Next to the
input, `ClassificationPicker` lets a user release the next message (issue
133): to a lower level than the channel's, or - in a channel with
compartments - to further compartments from `GET /compartments`. It offers
only what may enter the channel, the server decides, and a release is for
one message. Messages whose classification differs from the channel's carry
a `released` badge.

Who may reclassify - the channel's owner the channel, the author or the
owner a message - changes it with `ClassificationDialog`: from the pencil
next to the channel's classification, or the shield among a message's
actions. A change that releases something says what it releases before it
is made. Reclassifications follow the global stream: a message takes its new
classification from `MessageReclassified`, and a reclassified channel
fetches its messages again, which lowering it may have changed.

### Members, moderator and notes

The member list is sorted by name and uses the members' handles for `@`
completion and for highlighting mentions; participants that cannot be
mentioned, such as approvers, have none. The channel's moderator is shown
above it, and for `mentions_only` the input says that agents answer only when
mentioned (`moderator.ts`).

Where `can_change` is set, the member list offers adding a participant of the
session - with how much of the history they may see (`AddParticipantDialog`,
issue 46) - and removing one. The channel is fetched again when the global
stream announces a join or leave for it.

A "Notes" tab next to the members shows the notes the channel's agents
published (`NotesList`, issue 17). They come with the snapshot and are
fetched again (`GET …/notes`) when the global stream announces
`AgentChannelStateChanged` for the channel.

### What an agent saw

Stored messages of agents whose context the user may read (`can_read_context`)
carry an eye icon. It opens the context panel (`ContextPanel`, issue 41):
the messages of the context and the prompt parts where they were given, a
leading system prompt on top (issue 47), the knowledge the turn read as
"Knowledge read" - a channel of another session, which an agent's memory
came from, is not named (issue 134) - and - collapsed - the tools the model was offered as
"Tools offered", each with its description and parameters (issue 137). The panel shows internal messages and tool results, which
the chat hides; `MessageView` renders a message the same way in both. A
context never changes, so what was fetched once is kept. See
[AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw).
