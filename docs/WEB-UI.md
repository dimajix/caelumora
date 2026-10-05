# Web UI

`caelumora-web-ui` is the browser frontend: React and TypeScript, built with Vite and styled with
Tailwind. It talks to the REST server only, over the REST API and its two event streams. This
document describes how it is built and run, how it talks to the server, and what it shows. The
server side of every request is described in [REST-SERVER.md](REST-SERVER.md).

## Running it

```bash
cd caelumora-web-ui
npm install
npm run dev
```

The development server proxies `/api` to the REST server on `http://localhost:3000`, stripping the
prefix (`vite.config.ts`); the REST server is started as described in the [README](../README.md).
`npm run build` type-checks and builds into `dist/`, `npm run lint` runs ESLint.

## What the UI decides, and what it does not

The UI shows what the server says and repeats no rule of its own:

*   **Permissions come from the server.** Whether the user may change a channel (`can_change`) or
    see what an agent saw (`can_read_context`) is part of the responses, decided by the runtime's
    views (see [SECURITY.md](SECURITY.md#views)). The UI offers an action only where it is set, and
    never works it out itself.
*   **Only lexical rules are mirrored.** Whether a line is a command (`command.ts`, mirroring
    `caelumora_model::command::parse_input`) and how a participant's handle is derived
    (`mention.ts`, mirroring `caelumora_model::mention`) are repeated here, because the UI needs
    them while the user types. A handle is the name's ASCII form, made by the npm package
    `any-ascii` in the version the server's `any_ascii` crate has - both are pinned, so that their
    tables agree. What a command means and whom a mention addresses is decided by the server.
*   **The types mirror the server.** `src/types` holds the request and response types of
    `caelumora-rest-server/src/models.rs`; a change to the REST API changes them too.

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
| `src/moderator.ts`, `src/confidentiality.ts` | Labels and explanations of responder policies and classifications |
| `src/App.tsx` | Login state, keep-alive, the error banner, the layout |
| `src/components/` | `Login`, `Sidebar`, `ChatView`, `MessageView`, `MarkdownMessage`, `ContextPanel`, `HistoryPanel`, `ToolsPanel`, `ToolSetList`, `DeletedList`, `NotesList`, `TokenGauge`, `HoverCard`, `UsageStats`, `NewChannelDialog`, `AddParticipantDialog`, `PrivacyDialog`, `ErrorBanner` |

## Talking to the server

### Login and keeping the session

The session is an `HttpOnly` cookie the UI never sees; `api` sends it with every request. A `401`
brings up the login page.

A user who only reads would be logged out mid-conversation, because the event streams cannot renew
the token (see [REST-SERVER.md](REST-SERVER.md#renewal)). So `session.ts` renews by itself with
`POST /auth/refresh` while its tab is visible, and only then - a tab in the background is not
someone watching (issue 31). When to renew is read from the `caelumora_session_expires` cookie,
which also tells a tab about a renewal another tab made.

### Errors

The server answers errors with problem details (see [REST-SERVER.md](REST-SERVER.md#errors)).
`ApiRequestError` carries their `detail` as message and the `request_id`, under which the full error
is in the server log. Failed operations are listed in the error banner, newest last, each with its
request id; dialogs show what the server says about a request they can correct - a name that is
taken, say - next to the field.

### The event streams

Both streams go through one `openEventStream` (`src/stream.ts`), so the snapshot, the reconnect and
the check of the session are handled in one place:

*   It always asks for a snapshot (`?snapshot=true`), and gets one again after every reconnect.
*   On `expired` it reconnects at once. The new request carries the renewed cookie, and its snapshot
    restores what happened in between, so the handover is invisible.
*   An unexpected break is left to the browser's own retry. A stream that is really closed checks
    the session (`GET /auth/me`), whose `401` brings up the login page if that was the cause.

Messages can arrive twice, in the snapshot and as an event; they are dropped by id.

## What it shows

### The session list

The sidebar lists the user's sessions with their channels. It follows the global stream (issue 35,
`globalEvents.ts`): it takes its state from the snapshot, removes a session on `SessionDeleted`,
takes a new name from `SessionUpdated`, and fetches a session again (`GET /sessions/{id}`) on any
other `SessionUpdated` - a changed privacy - and on any event that changes its channels or their
members. Starting and stopping are ignored.

A session's privacy (issue 134, `privacy.ts`) is shown to everyone in it: a session other sessions
may read is marked in the list, and its menu opens `PrivacyDialog`, where its owner (`can_change`)
changes it. Choosing a policy that lets other sessions read says that it holds for what is said from
then on.

The user's name at the foot of the sidebar opens `ProfileDialog` (issue 176), where they write how
they introduce themselves to the agents (`GET`/`PUT /profile`). It counts the characters against the
limit the server names, and says that every agent in every channel they take part in is given it.

What the user deleted stays in the trash until it is purged (issue 164, `DeletedList`): below the
sessions a folded "Deleted sessions", and in every unfolded session a folded "Deleted channels",
each loaded when unfolded and again on `SessionDeleted`, `SessionRestored`, `ChannelDeleted` and
`ChannelRestored`. An entry shows who deleted it when, and offers "Restore" where `can_restore`
and "Delete for good" - confirmed by a second click - where `can_purge`; a channel only with its
session. A restored session or channel is fetched again like a new one.

A new session is created from a template; the one used last in this browser is preselected (issue
40). A new channel is created with its name, responder policy, classification - a level and, if any
are configured, compartments from `GET /compartments` (issue 133) - and participants
(`NewChannelDialog`, issue 09).

### A channel

`ChatView` shows one channel. Only a participant follows it live, over the channel's stream; anyone
else who may read it - the session's owner, say - is shown its history and no input.

*   **History.** The snapshot holds the last 20 messages; older ones are fetched on request. A
    message is shown with its tool calls, their status, their result and how long they took
    (`MessageView`); a call that is not public is marked with how far it may travel (issues 54, 61,
    100). A message's reasoning, where the server hands it out, is folded above its content under
    "Thought" and opened by the user (issue 216).
*   **While an agent answers**, its partial message is shown as it streams, its reasoning
    included: folded under "Thinking" until the answer begins (issue 216). Several agents answering
    at once each have an answer of their own, in the order they began, and a plan step of their own
    above the chat (issue 203). Answers under way are shown below every complete message: a
    message joins the history when it is complete, so an answer that ends first moves up past those
    still under way - as a reload shows the history. The stop button stays while any agent
    answers. Reasoning that is the
    owner's never reaches the chat, and reasoning held back while the agent could still make a call
    whose content is the owner's arrives only with the complete message
    (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reasoning)). A failed turn and the channel-wide
    answer of a command are shown in the chat but never stored. When a participant is removed, the
    channel says so with `ParticipantRemoved`, and nothing of it follows: its partial answer and the
    stop button go, and its calls still without a result are shown as abandoned (issue 162).
    When a chain between agents is cut off at its limit (`ChainCutOff`), a notice "Chain cut off"
    says so, naming the agents mentioned that were not asked, and that a message of a user starts a
    new chain; like a failed turn, it is gone on reload (issue 200).
*   **Approvals.** A tool call that needs approval is shown with approve and deny. Once this user
    answered, it waits for the decision; it disappears only when the approver ends it - decided,
    timed out, or withdrawn by an agent whose turn ended (issue 148). The pending approvals come
    with the stream's snapshot, so a reload shows them again; a request that arrives again - asked
    anew by an agent after a restart - is shown once (issue 172). Whether this user answered is
    known only to this page: after a reload, an answered approval is offered again.
*   **Stop.** The stop button ends the current turn of every agent (`POST …/cancel`).
*   **Input.** A line starting with `/` is sent as a command, `//` as a literal slash
    (`command.ts`). Typing `@` completes the members' handles, `/` the commands the user may see. An
    unsent draft survives a forced re-login, in the session storage. A message can be sent while
    an agent answers (issue 202): an agent in a turn hears it once the turn ends and answers it in
    a turn of its own, if asked to; the turn under way does not see it.

### Classification

The channel's classification is shown above the member list. Next to the input,
`ClassificationPicker` lets a user release the next message (issue 133): to a lower level than the
channel's, or - in a channel with compartments - to further compartments from `GET /compartments`.
It offers only what may enter the channel, the server decides, and a release is for one message.
Messages whose classification differs from the channel's carry a `released` badge.

Who may reclassify - the channel's owner the channel, the author or the owner a message - changes it
with `ClassificationDialog`: from the pencil next to the channel's classification, or the shield
among a message's actions. A change that releases something says what it releases before it is made.
Reclassifications follow the global stream: a message takes its new classification from
`MessageReclassified`, and a reclassified channel fetches its messages again, which lowering it may
have changed.

### Members, moderator and notes

Under each agent, a bar shows how full its context is (issue 209): the tokens of its last call
against the size of its context, amber from three quarters on and red from nine tenths
(`TokenGauge`); without a known size, the tokens alone. Its hover card keeps two things apart: the
context of the last call, against the context's size, and the usage of all of the agent's calls in
the channel as total input and total output tokens - totals, which grow much faster than the
context, since every call is given the whole context again. Above the list, "Total tokens" adds up
the usage of the agents shown. The numbers come with the stream's snapshot, and an agent's are
fetched again when its turn ends (`GET …/participants/{aid}`); an external agent reports none. A
message's "(i)" shows the input and output tokens of its call, how full the call's context was,
and its times.

All three show their numbers in the same hover card (`HoverCard` with `UsageStats`, issue 219):
groups under a heading, labels on the left and values aligned on the right, never wrapped, the card
as wide as its content. It opens on hover, on keyboard focus and on tap, above its trigger - below
where there is no room - and always inside the window; it is placed in the page's body, so that the
scrolling member list and chat do not cut it off, and closes when the page scrolls.

The member list is sorted by name and uses the members' handles for `@` completion and for
highlighting mentions; participants that cannot be mentioned, such as approvers, have none. Who
answers - the channel's `responder_policy` (issue 174) - is shown above it as its moderator, and for
`mentions_only_policy` the input says that agents answer only when mentioned (`moderator.ts`) - by
a user or by another agent.

Where `can_change` is set, the member list offers adding a participant of the session - with how
much of the history they may see (`AddParticipantDialog`, issue 46) - and removing one. The channel
is fetched again when the global stream announces a join or leave for it.

An agent whose tools the user may see (`can_read_tools`, issue 194) has a wrench icon in the member
list. It opens the tools panel (`ToolsPanel`): what the agent can call in this channel now
(`GET …/actors/{aid}/tools`), by the tool set the tools come from, with each set's description and,
where it is unavailable, why - each tool marked where a call needs approval, is shown to its owner
only, or has its results classified. The sets are drawn by `ToolSetList`, as in the context panel.
An agent that has no tools here - not running, or external - says so.

An agent the user may delete (`can_delete`) has a delete button in the member list; it asks once
more ("Delete agent?"), since the agent leaves every channel it takes part in, not only this one,
and a restore - only over REST so far - brings it back into none (issue 164).

Below the members, a folded group "Former members" lists who took part and left (issue 164), each
with when, and - where `can_read_context` - the history icon, so that what a removed agent did, the
turn it gave up when it was removed included ([146](../issues/146-show-given-up-turns.md)), can
still be read.

A "Notes" tab next to the members shows the notes the channel's agents published (`NotesList`, issue
17). They come with the snapshot and are fetched again (`GET …/notes`) when the global stream
announces `AgentChannelStateChanged` for the channel.

### What an agent saw

Stored messages of agents whose context the user may read (`can_read_context`) carry an eye icon. It
opens the context panel (`ContextPanel`, issue 41): the messages of the context and the prompt parts
where they were given, a leading system prompt on top (issue 47), the knowledge the turn read as
"Knowledge read" - a channel of another session, which an agent's memory came from, is not named
(issue 134) - and - collapsed - the tools the model was offered as "Tools offered", grouped by the
tool set they came from with its description and, where it was unavailable, why (issue 194), each
tool with its description and parameters (issue 137) and marked, as in the tools panel, where a call
needed approval, was shown to its owner only, or had its results classified. Every prompt part,
message and knowledge read shows how it was classified - "counts as the channel" where that is not
known - and what set the message's label, which the panel's head names, is marked in amber (issue
170). The panel shows internal messages and tool results, which the chat hides, and the reasoning of
the messages the model was given, marked "owner only" where it is the owner's; `MessageView` renders
a message the same way in both. For an answer of an external agent the panel shows the messages the
remote agent was given in the call that produced it, and nothing else (issue 205): no prompt parts,
tools or knowledge, and not what it was given in earlier calls. The panel's head words it so that it
holds for both kinds of agent ("What … was given"; the label set by what it had seen, marked what of
this set it): the REST API does not tell them apart. A context never changes, so what was fetched
once is kept. See [AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw).

The member list gives every agent whose context the user may read a history icon. It opens the
history panel (`HistoryPanel`, issue 207): all that was offered to the agent in the channel and what
it said itself (`GET …/actors/{aid}/history`), which may differ from what the channel shows. What
its perceive processors did not pass on to its transcript is greyed out and marked "not passed on";
a message whose context is recorded opens it in the context panel, and closing that returns to the
history. The history is loaded again whenever the channel's messages change.

The turns the agent gave up (issue 146, `GET …/actors/{aid}/given-up-turns`) are folded into the
history where they branched off: after the message the server names as `after`, first if it names
none, last if the history does not hold it. Each is a folded block with why it ended - failed,
stopped, or the channel lowered - and when; opened, it shows the turn's messages with their tool
calls and results, each with its context where it is recorded. A failed turn's notice in the chat
offers "What the turn did" for an agent whose context the user may read, which opens the history
panel with the latest turn given up unfolded. A turn given up because the agent was removed from
the channel is shown in its history panel, offered among the former members (issue 164).
