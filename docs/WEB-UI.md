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
| `src/panelWidth.ts` | The widths of the panels beside the chat, kept in browser storage |
| `src/moderator.ts`, `src/confidentiality.ts`, `src/workspace.ts` | Labels and explanations of responder policies, classifications and where a workspace lies |
| `src/App.tsx` | Login state, keep-alive, the error banner, the layout |
| `src/components/` | `Login`, `Sidebar`, `ChatView`, `MessageView`, `ToolCallView`, `QuestionCard`, `QuestionView`, `MarkdownMessage`, `ContextPanel`, `HistoryPanel`, `ToolsPanel`, `PrivateNotesPanel`, `ToolSetList`, `DeletedList`, `NotesList`, `TokenGauge`, `HoverCard`, `ActionMenu`, `ResizeHandle`, `UsageStats`, `NewSessionDialog`, `NewChannelDialog`, `AddParticipantDialog`, `PrivacyDialog`, `ApprovalRulesDialog`, `ErrorBanner`, `WorkspacePage`, `FileBrowser`, `ChannelWorkspacePanel`, `NewWorkspaceDialog`, `WorkspaceClassificationFields`, `SessionWorkspaceDialog` |

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

### Panel widths

The panels beside the chat - the sidebar, the members of a channel, and the panel that shows an
agent's context, history or tools in their place - can be made wider or narrower (issue 231): their
edge towards the chat is a handle (`ResizeHandle`), dragged with the mouse, a finger or a pen, or
focused and moved with the arrow keys (Shift for larger steps, Home and End for the narrowest and
widest); a double click, or Enter, returns it to the width it starts with. Each panel has a
narrowest and a widest width, and takes at most two fifths of the window, so that the chat keeps
room. The handle sits beside the panel, not in it, so that it does not scroll with the members
list.

The width is kept per panel - the context, history, tools and private notes panels share one - in
this browser's local storage (`usePanelWidth`, `src/panelWidth.ts`), not in the user's profile: the
right width depends on the screen. Without storage the panels start at their usual width every time.

### The session list

The sidebar lists the user's sessions with their channels. It follows the global stream (issue 35,
`globalEvents.ts`): it takes its state from the snapshot, removes a session on `SessionDeleted`,
takes a new name from `SessionUpdated`, and fetches a session again (`GET /sessions/{id}`) on any
other `SessionUpdated` - a changed privacy - and on any event that changes its channels or their
members. Starting and stopping are ignored.

What can be done with a session - rename, privacy, delete - and with a channel - delete - is in a
menu behind three dots at the end of its row, shown while the row is hovered (`ActionMenu`, issue
230). The menu is used the same way wherever something in a list has actions: it lists only what
may be done, and nothing - no button - where nothing may; it is reached with the keyboard too
(arrow keys, Escape returns to the button); an entry that cannot be undone may ask once more in its
place before it acts. Like the hover card it is placed in the page's body and inside the window
(`floating.ts`), so that no scrolling list cuts it off. Deleting a session or a channel asks once
more ("Delete session?", "Delete channel?"), though it only goes to the trash.

A session's privacy (issue 134, `privacy.ts`) is shown to everyone in it: a session other sessions
may read is marked in the list, and its menu opens `PrivacyDialog`, where its owner (`can_change`)
changes it. Choosing a policy that lets other sessions read says that it holds for what is said from
then on. The menu's "Approval rules" opens `ApprovalRulesDialog` (issue 13): the session's rules,
which its owner changes.

The user's name at the foot of the sidebar opens `ProfileDialog` (issue 176), where they write how
they introduce themselves to the agents (`GET`/`PUT /profile`). It counts the characters against the
limit the server names, and says that every agent in every channel they take part in is given it.
There, too, they choose the language they prefer (issue 56), which the channels they create are held
in. A language is typed as a tag or picked from suggestions (`LanguageField`, `src/language.ts`),
and shown by the name the browser knows for it (`Intl.DisplayNames`); whether it is a tag is the
server's to decide. Below, they see their rules on tool calls (issue 13), each with where it reaches
and until when, and take them back.

What the user deleted stays in the trash until it is purged (issue 164, `DeletedList`): below the
sessions a folded "Deleted sessions", and in every unfolded session a folded "Deleted channels",
each loaded when shown and again on `SessionDeleted`, `SessionRestored`, `ChannelDeleted` and
`ChannelRestored`. A list is shown only while it holds an entry (issue 232): the server leaves out
what the user may not read, so an empty list means nothing to see, whichever the reason, and it
disappears as soon as its last entry is restored or purged. An entry shows who deleted it when, and
offers "Restore" where `can_restore` and "Delete for good" - confirmed by a second click - where
`can_purge`; a channel only with its session. A restored session or channel is fetched again like
a new one.

"New Session" opens `NewSessionDialog` (issue [266](../issues/266-workspace-on-new-session.md)): the
template - the one used last in this browser preselected (issue 40) -, the name, "New \<template\>"
until the user changes it, and the workspace: as the template says, where it names one - its name
and where it lies, from `GET /templates` -, none, one of the user's own - not one made for another
session -, or a new one for the session alone, in a storage the user chooses, always offered, preset
to the template's or the first, and classified as they say. Everything is preset, so that Enter
makes a session as its template has it; a workspace chosen takes the place of the template's. What
the server refuses - a workspace not the user's, a classification they are not cleared for - is
shown in the dialog. A new channel is created with its name, responder policy, classification - a
level and, if any are configured, compartments from `GET /compartments` (issue 133) - language,
preset to the one the user prefers (issue 56), whether it uses the session's workspace, preset to
yes (issue 253), and participants (`NewChannelDialog`, issue 09).

### Workspaces

The user's own workspaces (issue [253](../issues/253-workspace.md)) are listed below the sessions,
each with where it lies where that is not their own scope, and a folded "Deleted workspaces" - the
trash, as for sessions - below them; the list is loaded again on every global event about a
workspace (`WorkspaceCreated`, `…Updated`, `…Deleted`, `…Restored`). "+" opens `NewWorkspaceDialog`:
its name, the storage from those the server names - each with the kind of workspace it makes,
files only or files and a shell (issue 266) -, whether its name lies in the user's scope or the
global one, what it holds and its classification (`WorkspaceClassificationFields`, shared with
`NewSessionDialog`) - what the server refuses, a name taken or a
classification the user is not cleared for, is shown in the dialog.

A workspace chosen there takes the place of a channel (`WorkspacePage`): what it is, the menu of
what its owner may do - rename, describe, reclassify, delete - and its files (`FileBrowser`). One
that lies in a session's scope is used within that session alone, and the page says so instead of
showing its files.

`FileBrowser` shows one directory at a time, directories first, with a path back to the top. A file
is downloaded through a plain link - the cookie goes with it, and the server sends it as an
attachment; files are uploaded into the directory shown, one request each, the file as the body;
deleting asks once more in the menu. It works on a `WorkspaceFiles` of the API client, which is the
owner's way to the files (`api.ownedFiles`) or a channel's (`api.channelFiles`) - the browser does
not know the difference.

A session's menu has "Workspace" (`SessionWorkspaceDialog`): the workspace attached, if the user
may read it - its owner -, and to whoever may change the session one of their workspaces to attach,
not one made for another session, or taking it away.

### A channel

`ChatView` shows one channel. Only a participant follows it live, over the channel's stream; anyone
else who may read it - the session's owner, say - is shown its history and no input.

*   **History.** The snapshot holds the last 20 items of the history; older ones are fetched on
    request (`GET …/history`). Its messages are shown, and a turn given up as the notice the chat
    shows live, named for its turn so that it is shown once (`chatMessages`, `givenUpNotice`, issue
    258); a turn that finished shows nothing of its own. A
    message is shown with its tool calls, their status, their result and how long they took
    (`MessageView`); a call that is not public is marked with how far it may travel (issues 54, 61,
    100). A message's reasoning, where the server hands it out, is folded above its content under
    "Thought" and opened by the user (issue 216).
*   **Tool calls** are shown by their presentation (issue 165), which the tool declares and the
    agent stamps on each call, never by the tool's name. `ToolCallView` draws the frame every call
    has - the tool, who sees it, its status and duration - and leaves what is inside to the handler
    of the call's presentation (`PRESENTATIONS`); a call without one, or with one this client does
    not know, gets the default handler: its arguments and its result as they are. The `shell`
    handler (issue 251) shows a command as a line of code, what it printed as a terminal shows it,
    and its exit code, or the id it runs under; a call that follows or ends a command names its id.
    The `file_read`, `file_write` and `file_edit` handlers show the file's path and its text as it
    is - its lines kept, a long line wrapped at word boundaries, a long text scrolling in its box:
    what was read, with which lines of how many and where to read on; what was written, and how
    many bytes; the text replaced and the text put in its place, and how many places. The
    `file_list` and `file_search` handlers (issue 265) show a directory listed - its path, its
    entries as the file browser shows them, and whether the listing was cut off - and a search -
    its pattern and where, its matches by file with their line numbers, and whether it stopped
    early or skipped files. The `file_move` handler (issue 267) shows where a file or directory lay
    and where it lies now, and whether it was renamed or moved. The `artifact_publish` handler
    (issue 271) shows a published version as a card like `file_publish`'s (`DownloadCard`) - the
    artifact's name, version, size and description, a download of that version through the
    channel (`artifacts`, handed as `files` is), and "Keep" for its owner while it lies in the
    session's scope, which it learns by asking for the artifact as theirs. The `file_publish`
    handler (issue 273) shows a file offered for download as a card - its name, size and what it
    is - with a download through the channel's workspace (`files`, which `ChatView` hands through
    `MessageView`), and says when the file changed since it was offered - its size or when it was
    changed differ - or is gone; whoever cannot use the workspace there, and every view outside
    the chat, gets the card without the download. The `plan` handler shows a plan's summary and
    its steps, numbered (issue 263).
    Where a result keeps the record of the questions its call asked (`answers`, issue 165), the
    frame shows it (`AnswersRecord`, issue 268): who answered or declined, and each question with
    every choice that was offered, one below the other, the chosen ones marked and the others
    muted, "Other" with its text, a written answer as a quote, a skipped question as such. A
    question's text keeps its line breaks, so that an approver reads a command of several lines as
    it was written. The `questions` handler of `ask_user` shows nothing raw: while it waits, its
    questions as they were put and whom they wait for; once answered, nothing beyond the record.
*   **While an agent answers**, its partial message is shown as it streams, its reasoning
    included: folded under "Thinking" until the answer begins (issue 216). Several agents answering
    at once each have an answer of their own, in the order they began (issue 203). Answers under
    way are shown below every complete message: a message joins the history when it is complete,
    so an answer that ends first moves up past those still under way - as a reload shows the
    history. The answer of a step of a plan (`IntermediatePartialGenerated`) streams in within its
    step's block instead, after the step's messages; only where its turn has no such block - a
    snapshot without the step's mark - is it shown below (issue 263). Each answer under way knows
    its turn, from `ResponseStarted` or, for one that began before the page was loaded, from the
    snapshot's `turn_under_way` of its member (issues 235, 240); what streamed before is not
    shown. The stop button stays while any agent answers. Reasoning that is the owner's never
    reaches the chat, and reasoning held back while the agent could still make a call whose
    content is the owner's arrives only with the complete message (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reasoning)). A turn given up (`TurnGivenUp`, issue 258)
    is shown in the chat with why: a failure as an alert naming what failed, any other reason as a
    notice - the channel lowered, the answer asked for again; it is not stored yet and gone on
    reload. The channel-wide answer of a command is shown but never stored. A command an agent
    carried out (issue 269) is shown as the agent's notice - with what it answered, live, to the
    issuer or to everybody -, kept once by its turn, and from the history after a reload without the
    answer; one it did not carry out is a notice for the issuer alone. An agent removed while
    it answers gives its turn up and says so, which is shown as any turn given up (issue 262); once
    it has ended, the channel says so with `ParticipantRemoved`, and nothing of it follows: what
    an agent aborted without a word left - its partial answer, the stop button, its calls still
    without a result - goes or is shown as abandoned (issue 162).
    When a chain between agents is cut off at its limit (`ChainCutOff`), a notice "Chain cut off"
    says so, naming the agents mentioned that were not asked, and that a message of a user starts a
    new chain; like the answer of a command, it is gone on reload (issue 200).
*   **A plan** (issue 263) is shown by its call: `create_plan` is presented as `plan`, its summary
    and its steps numbered, a plan extended as a call of its own.
*   **The steps of a plan** (issues 258, 263). While an agent works through a plan, what each step
    says - its answer, its tool calls and their results - arrives as `IntermediatePartialGenerated`
    and `IntermediateMessageGenerated` and is shown as any partial and complete message is, a tool
    result refreshing the workspace alike. Each step is a block of its own (`PlanSteps`,
    `chatRows`), headed "works on step 2 of 3" with the step's description: `PlanStepStarted`, live,
    and the history's `plan_step_started`, after a reload, become a mark in the chat's messages
    (`planStepMark`) that begins the block, and the internal messages of its turn that follow join
    it. The step a turn is in is open while the turn is under way, every other step folded, unless
    the user opened or closed it. Messages of a turn interrupted by another agent's continue in a
    block of their own under the same step; internal messages before any mark - a history from
    before steps were recorded, or a snapshot cut within a step - are headed "the plan". The
    messages are rendered as every other message, with their icons; none of them is offered for
    another answer.
*   **Changing one's own messages** (issue 234). A participant's own message has an edit icon, which
    turns it into an editor in its place (`MessageEditor`: save, cancel, Escape, Ctrl+Enter), and a
    take-back icon, which asks "Take back?" on the first click and takes the message back on the
    second (`PUT` and `DELETE …/messages/{mid}`). A modified message shows "edited" in its footer
    with when on hover; one taken back is a placeholder naming who took it back. Both arrive as
    `MessageModified` and `MessageRevoked` - from others, and from the user's own actor - and are
    applied to the message in place as the server reads them, so a reload shows the same. The
    classification of a modified message stays as it is; it is changed as before, with the shield
    icon.
*   **Asking for another answer** (issue 234). The last message of the chat, if an agent's, has a
    retry icon while nothing is being generated and its member's `can_retry` allows it (`POST
    …/messages/{mid}/retry`) - not of an external agent, which never answers again (issue 242). The
    agent decides: on `RetryAccepted` it is shown as answering, its message turns into a placeholder
    as it takes it back, and its new answer follows; on `RetryRejected` a notice "Not answered
    again" says why, in this page's words for each `reason`, and is gone on reload. An answer still
    under way has a retry icon of its own once its turn is known and its
    member's `can_retry` allows it (issue 235): `POST …/participants/{aid}/retry` with the turn.
    Refused if the answer ended meanwhile - its last message then has the icon. On `RetryAccepted`
    of a turn, the messages of that turn shown so far stay: what was said is not unsaid, and the
    history keeps them (issue 258). The notice of a turn given up because it failed or the channel
    was lowered has the retry icon too, while it is the last thing in the chat, nothing is being
    generated and the member's `can_retry` allows it (issue 259): the same route with that turn,
    and the agent answers anew after it.
*   **Approvals.** A tool call that needs approval is shown to everybody, with approve and deny only
    for who may approve the agent's calls (`can_approve` of the member, issue 14); everybody else is
    told that it waits for whoever is responsible for the agent. Once this user answered, it waits
    for the decision; it disappears only when the approver ends it - decided, timed out, or
    withdrawn by an agent whose turn ended (issue 148). The pending approvals come with the stream's
    snapshot, so a reload shows them again; a request that arrives again - asked anew by an agent
    after a restart - is shown once (issue 172). Whether this user answered is known only to this
    page: after a reload, an answered approval is offered again. Unless a rule of the session or
    channel asked for it (`explicit`), a card offers "and from now on" (`ApprovalRuleOffer`, issue
    13): a rule derived from the call - its patterns from `suggested`, editable - for this
    channel, this session or everywhere, until taken back or for an hour or a day; it is added
    after the answer, with the answer's decision. A request a rule of this user's answered says so
    ("Answered by your rule"), from their own answer as their actor shows it.
*   **Questions.** The questions an agent's call waits for the answers to (issue 165) are shown to
    everybody the server sends them to, above the approvals (`QuestionCard`): each question with
    its header, its choices one below the other, each a row of the full width with what it means -
    a circle where one is chosen, a box where several may be, the recommended one marked (issue
    268) - an "Other" field below them, or a text field without choices. The card and the record of
    the answers draw a question alike (`QuestionView`). Only the users they are put to get
    the controls, everybody else whom they wait for; a question may be skipped, the whole of them
    declined. Sensitive questions mask the input and say that the answers go to the tool alone;
    the others that they go to the agent, and that writing a message instead declines them. Sent
    with `POST …/questions/{qid}`, a card says "Answered" until the agent closes the questions
    (`QuestionClosed`); the open questions come with the stream's snapshot.
*   **Brave mode.** "Approve everything in this session for an hour" adds a rule that allows every
    tool call there for an hour (`approvalRules.ts`); while one of this user's rules allows
    everything in the channel, a bar says so, until when, and stops it. The user's rules follow
    the global stream (`UserProfileUpdated`).
*   **Stop.** The stop button ends the current turn of every agent (`POST …/cancel`). The stop
    icon on an answer under way ends that agent's turn alone (`POST …/participants/{aid}/cancel`,
    issue 235); the other answers go on.
*   **Input.** A line starting with `/` is sent as a command, `//` as a literal slash
    (`command.ts`). Typing `@` completes the members' handles, `/` the commands the user may see -
    the channel's, and the agents' with each handle that carries one out and `@all` (issue 269). An
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
A message takes its new classification from `MessageReclassified` on the channel's stream, as
every change to a message (issue 239); a reclassified channel follows the global stream and fetches
its messages again, which lowering it may have changed.

### What the channel is for

The panel beside the chat is the channel panel - its tab is called "Channel" (issue 245). At its
top stands the channel's summary and, behind "Briefing", its briefing rendered as Markdown
(`ChannelDescription`). Who may change the channel changes both with `DescriptionDialog`, from the
pencil next to the summary, or from "Say what this channel is for" where it says nothing yet; the
dialog says who reads it. A change made elsewhere arrives as `ChannelUpdated` with
`DescriptionChanged`, which carries both.

A session's description is opened from its menu in the sidebar ("Description"), with the same
dialog - read-only for whoever may not change the session - which says there that everybody in the
session and every agent in its channels reads it, external ones too. The session is fetched first:
the list holds no briefing, and saving without it would remove it. The summaries of sessions and
channels are their tooltips in the sidebar.

### Language

The channel's language is shown below its classification (issue 56). Who may change the channel
(`can_change`) changes it or takes it back with `LanguageDialog`, from the pencil next to it; the
dialog says that nothing said before is translated. A change made elsewhere arrives as
`ChannelUpdated` with `LanguageChanged`.

### Approval rules

Below the language, the channel's approval rules are counted (issue 13), for whoever may change the
channel and, once there are some, for everyone; the count opens `ApprovalRulesDialog`, the same
dialog as the session's. A rule names a tool - `*` for every one - and optionally a pattern per
argument, and asks or denies; the dialog offers nothing that allows, since a place cannot. The
rules are saved as a whole, and what the server refuses of them is shown in the dialog. An
approval asked for by such a rule says so on its card (`explicit`).

### The channel's files

Beside "Channel" and "Notes", the channel panel has "Files" (`ChannelWorkspacePanel`): the session's
workspace as the user may use it in this channel (`GET …/workspace`) - its name, classification and
what it holds - and its files, through the channel; or that no workspace is used in this channel,
without why. The files are loaded again when the tab is shown, when a tool call of the channel
ended (a `CompleteMessageGenerated` with tool results) and when the channel's switch changes. The
"Channel" tab shows the switch - "Workspace: used here" - as a checkbox to whoever may change the
channel, as text to everybody else (issue 251).

### Members, moderator and notes

Under each agent, a bar shows how full its context is (issue 209): the tokens of its last call
against the size of its context, amber from three quarters on and red from nine tenths
(`TokenGauge`); without a known size, the tokens alone. Its hover card keeps two things apart: the
context of the last call, against the context's size, and the usage of all of the agent's calls in
the channel as total input and total output tokens - totals, which grow much faster than the
context, since every call is given the whole context again. Above the list, "Total tokens" adds up
the usage of the agents shown. The numbers come with the stream's snapshot; an agent's are updated
by each of its calls as its message arrives - the steps of a plan are many calls of one turn - and
fetched again when its turn ends or is given up (`GET …/participants/{aid}`), which also counts
calls whose messages were not published. An external agent reports none. A message's "(i)" shows
the input and output tokens of its call, how full the call's context was, and its times.

All three show their numbers in the same hover card (`HoverCard` with `UsageStats`, issue 219):
groups under a heading, labels on the left and values aligned on the right, never wrapped, the card
as wide as its content. It opens on hover, on keyboard focus and on tap, above its trigger - below
where there is no room - and always inside the window; it is placed in the page's body, so that the
scrolling member list and chat do not cut it off, and closes when what holds its trigger scrolls -
not when the chat beside it receives messages. It is placed by `useFloating` (`floating.ts`), like
the menus, and placed again when its size changes.

Each agent has a dot for where it stands (issue 243), its state as the dot's title: green when
idle, green pulsing in colour and size while it works (`Planning`, `ExecutingTools`, `Reflecting`,
`Cancelling`), amber while it waits for an approval, a still blue while it sleeps, and grey where
nothing runs - the channel does not run, or the agent is gone. Users and approvers have none. The
dot starts from the member's `state` in the snapshot and follows `StateChanged`. An agent whose
answer a cancel can still end - working or waiting for an approval - has a stop icon where its
member's `can_cancel` allows it (`POST …/participants/{aid}/cancel`); so has its answer under way in
the chat, offered to everybody before.

The member list is sorted by name and uses the members' handles for `@` completion and for
highlighting mentions; participants that cannot be mentioned, such as approvers, have none. Who
answers - the channel's `responder_policy` (issue 174) - is shown above it as its moderator, and for
`mentions_only_policy` the input says that agents answer only when mentioned (`moderator.ts`) - by
a user or by another agent.

A member's row shows its name and handle, and below them how full its context is; what can be done
with the member is in its menu (`ActionMenu`, issue 230) - "Show history", "Show private notes",
"Show tools", "Remove from channel", "Delete agent" - each only where the member offers it, and no
menu where it offers nothing.

Where `can_change` is set, the member list offers adding a participant of the session - with how
much of the history they may see (`AddParticipantDialog`, issue 46) - and the menu removing one.
The channel is fetched again when the global stream announces a join or leave for it.

Every member but the approver has "Show profile" in its menu (issue 244). It opens the profile
panel (`ProfilePanel`), filled once by `GET …/participants/{aid}`: name and handle; of an agent the
name of its personality, whether it runs here or elsewhere - where what it is given leaves the
system - and who owns it; and its introduction, a user's as "in their own words", or that there is
none.

An agent whose tools the user may see (`can_read_tools`, issue 194) has "Show tools" in its menu.
It opens the tools panel (`ToolsPanel`): what the agent can call in this channel now
(`GET …/actors/{aid}/tools`), by the tool set the tools come from, with each set's description and,
where it is unavailable, why - each tool marked where a call needs approval, is shown to its owner
only, or has its results classified. The sets are drawn by `ToolSetList`, as in the context panel.
An agent that has no tools here - external, or its channel not running - says so.

An agent whose context the user may read (`can_read_context`) has "Show private notes" in its menu
beside "Show history" (issue 233). It opens the private notes panel (`PrivateNotesPanel`): what the
agent noted for itself in every channel of the session the user may read
(`GET /sessions/{id}/actors/{aid}/private_notes`), grouped by channel - this channel first, the
others by name - each note with when it was written and its classification, and with its number
only in this channel, where the agent is shown it. It is fetched again whenever the global stream
announces `AgentChannelStateChanged` for that agent in the session. Who may read an agent's context
sees its private notes there anyway (issue 54); the panel shows them current and on their own.

An agent the user may delete (`can_delete`) has "Delete agent" in its menu; it asks once more in its
place ("Delete agent? It leaves every channel"), since the agent leaves every channel it takes part
in, not only this one, and a restore - only over REST so far - brings it back into none (issue 164).

Below the members, a folded group "Former members" lists who took part and left (issue 164), each
with when, and - where `can_read_context` - "Show history" and "Show private notes" in its menu, so
that what a removed agent did, the turn it gave up when it was removed included
([146](../issues/146-show-given-up-turns.md)), and what it kept to itself can still be read.

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
170). The panel shows internal messages and tool results, which the chat folds, and the reasoning of
the messages the model was given, marked "owner only" where it is the owner's; `MessageView` renders
a message the same way in both. For an answer of an external agent the panel shows the messages the
remote agent was given in the call that produced it, and nothing else (issue 205): no prompt parts,
tools or knowledge, and not what it was given in earlier calls. The panel's head words it so that it
holds for both kinds of agent ("What … was given"; the label set by what it had seen, marked what of
this set it): the REST API does not tell them apart. A context never changes, so what was fetched
once is kept. See [AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw).

Every agent whose context the user may read has "Show history" in its menu in the member list. It
opens the history panel (`HistoryPanel`, issue 207): all that was offered to the agent in the
channel and what it said itself (`GET …/actors/{aid}/history`), which may differ from what the
channel shows. What its perceive processors did not pass on to its transcript is greyed out and
marked "not passed on"; a message whose context is recorded opens it in the context panel, and
closing that returns to the history. The history is loaded again whenever the channel's messages
change.

A turn the agent gave up (issues 146, 258) is marked where it ended, after what it said: why -
failed, stopped, the channel lowered, asked for again - and when, as the history's items say. Its
messages stand among the others, each with its context where it is recorded. A turn given up
because the agent was removed from the channel is shown in its history panel, offered among the
former members (issue 164). A command the agent carried out (issue 269) is marked where it was:
which, with its arguments, on whose word, and when.
