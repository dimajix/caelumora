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

The development server proxies `/api` to the program on `http://localhost:3000` (`vite.config.ts`),
which serves the API there; the program is started as described in the [README](../README.md).
`npm run lint` runs ESLint.

`npm run build` type-checks and builds into `dist/`, which `caelumora-app` serves from the binary:
embedded in a release build, read from `dist/` in a debug build, where a new build of the UI shows
without building the program again (see [REST-SERVER.md](REST-SERVER.md#the-program)). A build of
the program with the UI built in fails while `dist/` is empty; `CAELUMORA_BUILD_UI=1` has it build
the UI first.

## What the UI decides, and what it does not

The UI shows what the server says and repeats no rule of its own:

*   **Permissions come from the server.** Whether the user may change a channel (`can_change`), see
    what an agent saw (`can_read_context`), approve, retry, cancel or delete is part of the
    responses, decided by the runtime's views (see [SECURITY.md](SECURITY.md#views)). The UI offers
    an action only where it is set, and never works it out itself.
*   **Only lexical rules are mirrored.** Whether a line is a command (`command.ts`, mirroring
    `caelumora_model::command::parse_input`) and how a participant's handle is derived
    (`mention.ts`, mirroring `caelumora_model::mention`) are repeated here, because the UI needs
    them while the user types. A handle is the name's ASCII form, made by the npm package
    `any-ascii` in the version the server's `any_ascii` crate has - both are pinned, so that their
    tables agree. What a command means and whom a mention addresses is decided by the server.
*   **The types mirror the server.** `src/types` holds the request and response types of
    the crate `caelumora-rest-api`; a change to the REST API changes them too. The form serde
    gives an `ActorId` is read and made only in `actor.ts`, so that a change of it is met in one
    place.
*   **What the server refuses is shown where it can be corrected.** A dialog shows the server's
    refusal - a name taken, a classification the user is not cleared for - next to the field; it
    does not check beforehand what the server decides.

## Structure

| File | Holds |
|---|---|
| `src/api/index.ts` | Every REST call, relative to `/api`, with the session cookie |
| `src/types/index.ts` | The request and response types |
| `src/errors.ts` | Problem details as `ApiRequestError`, and how errors are reported |
| `src/session.ts` | Keeping the session alive; drafts and the last template in browser storage |
| `src/attachmentDraft.ts` | The attachments of a message being written, uploaded as they are attached |
| `src/stream.ts` | `openEventStream`, for both event streams |
| `src/globalEvents.ts` | What the session list does with a global event |
| `src/command.ts`, `src/mention.ts` | The lexical rules mirrored from the model |
| `src/actor.ts` | Who an `ActorId` names, and the `ActorId` of a user or an agent |
| `src/page.ts`, `src/pageList.ts` | What is shown in the place of a channel, and the lists in the sidebar it is opened from |
| `src/App.tsx` | Login state, keep-alive, the error banner, the layout |
| `src/components/` | One component per file, named after what it shows |

## Talking to the server

### Login and keeping the session

The session is an `HttpOnly` cookie the UI never sees; `api` sends it with every request. A `401`
brings up the login page.

A user who only reads would be logged out mid-conversation, because the event streams cannot renew
the token (see [REST-SERVER.md](REST-SERVER.md#renewal)). So the UI renews by itself with
`POST /auth/refresh` while its tab is visible, and only then: a tab in the background is not someone
watching. When to renew is read from the `caelumora_session_expires` cookie, which also tells a tab
about a renewal another tab made.

### Errors

The server answers errors with problem details (see [REST-SERVER.md](REST-SERVER.md#errors)).
`ApiRequestError` carries their `detail` as message and the `request_id`, under which the full error
is in the server log. Failed operations are listed in the error banner, newest last, each with its
request id.

### The event streams

Both streams go through one `openEventStream`, so the snapshot, the reconnect and the check of the
session are handled in one place:

*   It always asks for a snapshot, and gets one again after every reconnect.
*   On `expired` it reconnects at once. The new request carries the renewed cookie, and its snapshot
    restores what happened in between, so the handover is invisible.
*   An unexpected break is left to the browser's own retry. A stream that is really closed checks
    the session (`GET /auth/me`), whose `401` brings up the login page if that was the cause.
*   A channel's stream stays open while its channel stops and starts again, and opening a channel
    does not start it (issue 386). The snapshot sent at each stop and start replaces what was shown,
    so a participant's state and an answer under way are gone with the channel that ran them.

Messages can arrive twice, in the snapshot and as an event; they are dropped by id. A notice is
dropped by the position of its entry in the channel's history: an event carries it as `position`,
the item it is read as later as `seq` (issue 417).

What the user was shown is what the channel's history recorded for them, so a reload shows the
same: a turn given up, the answer of a command - the channel's and an agent's, a private one to its
issuer -, a command not carried out, a chain cut off, that it could not be decided who responds, a
nudge, a refusal to let something into the channel, a retry rejected (issue 417). A message of
theirs that was not let in is marked live; after a reload, the notice of its refusal stands in its
place, since the message entered nothing.

## What it shows

### Common elements

*   **Menus.** What can be done with something in a list is in a menu behind three dots
    (`ActionMenu`). It lists only what may be done, and there is no menu where nothing may; it is
    reached with the keyboard too. An entry that cannot be undone asks once more in its place.
*   **Hover cards and menus** are placed in the page's body and inside the window, so that no
    scrolling list cuts them off.
*   **Panel widths.** The panels beside the chat can be made wider or narrower by their edge, with
    pointer or keyboard. Each takes at most two fifths of the window, so that the chat keeps room.
    The width is kept per panel in this browser's local storage, not in the user's profile: the
    right width depends on the screen.
*   **What was deleted.** What the user deleted is listed folded below its list ("Deleted sessions",
    "Deleted channels", ...) until it is purged, each entry with who deleted it when, and restore
    and purge where the server allows them. A list is shown only while it holds an entry: the server
    leaves out what the user may not read, so an empty list means nothing to see, whichever the
    reason.

### The session list

The sidebar lists the user's sessions with their channels and follows the global stream: it takes
its state from the snapshot and fetches a session again on any event that changes it, its channels
or their participants. Starting and stopping are ignored. An event about an entity is read by its
kind (`entityEvent` in `globalEvents.ts`): the lists of workspaces, projects, skills, artifacts,
teams and agents are loaded again on any event of their kind, and what was deleted on any deletion
or restoring, whatever the kind.

What is new in a channel (issue 404) is shown beside it: how many messages of others came since the
user last read it, and an `@` where one of them mentions them; a folded session shows the sum of its
channels. The list takes the numbers from the snapshot and counts on with every message the global
stream announces (`said`) - except in the channel on screen while the tab is visible -, and the
chat view clears them as it marks the channel read (`unread.ts`). What another tab or device read
is seen here on the next listing. A message that mentions the user is shown as a notification of
the browser where they allowed it - asked for by the bell at the foot of the sidebar - unless its
channel is on screen.

A session's privacy is shown to everyone in it; its owner changes it (`PrivacyDialog`). Choosing a
policy that lets other sessions read says that it holds for what is said from then on. The session's
approval rules are changed by its owner from its menu. Deleting a session or a channel asks once
more, though it is only marked and can be restored.

The user's name at the foot of the sidebar opens their profile: how they introduce themselves to the
agents - with the note that every agent in every channel they take part in is given it - the
language they prefer, and their own rules on tool calls, each with where it reaches and until when,
to take back. Whether a language tag is valid is the server's to decide. The preferred language is
the web UI's to apply, not the server's (issue 331): the dialog for a new session presets it where
the template names no language. A session's language and time zone are changed from its menu.

"New Session" presets everything - the template used last in this browser, its name, the workspace
the template names, the language the template names or else the one the user prefers, the time
zone the template names or else the browser's - so that Enter makes a session as its template has
it. A workspace chosen there takes the place of the
template's: none, one of the user's own, or a new one for the session alone. A new channel is
created with its name, responder policy, classification, language (empty: the session's), whether
it uses the session's workspace (preset to yes), and participants.

### Workspaces

The workspaces the user may read - their own and those shared with them - are listed below the
sessions, each with where it lies where that is not the user's own scope. A workspace opens in the
place of a channel (`WorkspacePage`): what it is, what its owner may do with it, whom it is shared
with (`AccessList`), and its files - wherever it lies.

`AccessList` shows each grant with what it allows, to change or take away, and adds the members of a
session the user may read, or of one of its channels, or a team the user reads. Users and roles are
not offered: the REST API lists neither.

The lists of workspaces, projects, skills, artifacts, teams and agents in the sidebar are one
component (`PageList`).
A page shown in the place of a channel is closed when the list it was listed in loses its entry; one
the list never had - an artifact read only as a participant of a channel, opened from the chat -
stays.

The file browser downloads a file through a plain link - the cookie goes with it, and the server
sends it as an attachment - and uploads and deletes only for whoever may change the files
(`can_write`); who may only read is told so. It works always by the workspace, never through a
channel.

A session's workspace is attached or taken away from its menu by whoever may change the session: one
of their own workspaces, not one made for another session, shared with the session's members as
chosen.

### Projects

The shared projects the user reads (issue 467) - their own, those they work on and those shared with
them - are listed below the workspaces; a session's own is reached through its session. A project's
page (`ProjectPage`) shows what it is, its members, whom it is shared with, the sessions that lie in
it of those the user reads, and its artifacts. Its owner renames, describes, deletes it - with its
sessions - and chooses its members as a team's are chosen (`AddUser`). Whom the server keeps in its
access list - its members, the participants of its sessions - is shown there, not changed (`kept`).
A session's own project is called by its session (`projectLabel`, since every own one is named
`own`); its owner keeps it by lifting it into their name space or the global one under a name of its
own, while the session publishes into it.

A session's project is shown and changed from its menu (`SessionProjectDialog`) by whoever may
change the session: put into one of the shared projects the user reads, or taken out into its own. A
new session may be made in a shared project as well. Nothing moves: what was published stays where
it went.

### Skills

The skills the user may read and is cleared for are listed below the projects. A skill opens on a
page of its own, its text shown as Markdown; its owner renames, describes, reclassifies, deletes it
and edits its text in place. In the chat, `/skill` is shown as every command of an agent, and the
skill's text stands in the agent's history panel as what the issuer gave it. An agent's calls of the
skill tools are shown by their presentations: the skill by its name, linked to its page - never the
text a skill loaded gave the agent.

### Artifacts

The artifacts the user may read - their own and those shared with them by name - are listed below
the skills. Nothing is made there: an artifact is published by an agent. One read only as a
participant of the channel it was published in is not listed, but opened from its card in the chat.

An artifact's page shows what it is, the project it lies in - linked to the project's page -, its
classification, and its versions the user may read, each with where and by whom it was published -
by name for whoever may read that channel, "a channel you do not read" otherwise. Its owner
describes, shares and deletes it - sharing hands whom the list newly names every version there is
(issue 316) -, and sees whom each version reaches; whoever may add versions without owning it is
told so.

### Teams

The teams the user owns or belongs to are listed below the artifacts. A team's page shows its
members; its owner renames and deletes it and adds and removes members, every change writing the
whole list. Since the REST API lists no users, a member is chosen from the participants of one of
the user's sessions - so only someone the user shares a session with. Everyone else sees the team
read-only, as everyone sees the group of a session's or channel's participants, which changes only
by taking part.

### Agents

The agents the user may read are listed below the teams, each with where it lies where that is not
the user's own scope - an agent of a session's scope takes part only there and goes with it. An
agent's page shows its personality, where it lies, whose it is, and the channels it takes part in
that the user may read, each opened from there; whoever may renames and deletes it from its menu.
A deleted agent rests in its channels until it is restored from "Deleted agents" below the list.

"New agent" makes one of a personality the user may read, in their own scope: it is theirs, acts
with their rights and keeps what it learns from session to session. A personality of a session's
scope is not offered there, since it makes no agent outside it. An agent for one session is made
from the dialog that adds someone to a channel (see below).

### A channel

`ChatView` shows one channel. Only a participant follows it live, over the channel's stream; anyone
else who may read it - the session's owner, say - is shown its history and no input, and what
others say in it as the global stream announces it. A channel that does not run, its components
unreadable, says so and why above the input (`unavailable`); what is done there tries it again, and
it is fetched anew as it starts.

*   **What was read.** The chat marks the channel read up to the newest message of someone else it
    shows, while the tab is visible - as the channel is opened, as messages arrive and as the tab
    comes back. Never up to one of the user's own, which is shown before the channel takes it in,
    nor to a notice of the UI's. A line marks where the new messages began as the channel was
    opened, and stays there while the user reads; where the message last read lies before the pages
    shown, or nothing was read, it stands at the top.

*   **Following the conversation.** A channel opens at its end, and the chat follows what is said
    while the user is there; one who scrolled up to read stays where they are, until they scroll
    back down or send something themselves.
*   **History.** The snapshot holds the last page of the history; "Load earlier messages" puts the
    page before those shown in front of them, until there is none (issue 218). One who does not take
    part is shown the last page the same way. A reclassification of the channel shows its last page
    anew, as a snapshot would, and lets go of the older pages loaded. A turn given up is shown as
    the notice the chat shows live, once; a turn that finished shows nothing of its own. A call that
    is not public is marked with how far it may travel. A message's reasoning, where the server
    hands it out, is folded above its content.
*   **Whose a message is.** The user's own messages - those whose sender is their actor - stand on
    the right, in a colour of their own; everybody else's, other users' as well as agents', on the
    left, named above them. The role decides only that a person's message keeps its line breaks.
    The context and history panels colour messages by the same rule.
*   **What a turn said** stands together (issue 415): the messages of one turn in a row, and the
    notices of it, are one group, indented as a whole - its agent named only above the first,
    close to one another, the corner pointing at the agent only on the last. An answer under way
    goes on with the group of its turn. Turns of agents
    answering at once interleave, each part a group of its own; pointing at a message of a turn of
    more than one shades every part of it. A user's message belongs to no turn and stands alone.
*   **Tool calls** are shown by their presentation, which the tool declares and the agent stamps on
    each call, never by the tool's name. `ToolCallView` draws the frame every call has - the tool,
    who sees it, its status and duration, the record of questions it asked - and leaves what is
    inside to the handler of the presentation (`PRESENTATIONS`); a call without one, or with one
    this client does not know, shows its arguments and result as they are. What each handler shows
    is described with it in `ToolCallView.tsx`.
*   **While an agent answers**, its partial message is shown as it streams. Several agents answering
    at once each have an answer of their own, in the order they began, below every complete
    message: a message joins the history when it is complete, so an answer that ends first moves up
    past those still under way - as a reload shows the history. Each answer under way knows its
    turn, from its start or from the snapshot. Reasoning that is the owner's never reaches the chat,
    and reasoning held back while the agent could still make a call whose arguments are the owner's
    arrives only with the complete message (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reasoning)).
*   **What ends without an answer.** A turn given up is shown with why: a failure as an alert naming
    what failed, any other reason as a notice. A command an agent carries out is shown to the issuer
    as the agent's notice from when it begins, replaced by its answer or failure. What an agent does
    of its own accord - compacting its context - is shown to everybody as its notice from when it
    begins (`ActivityStarted`), replaced by what came of it (`ActivityFinished`), which the history
    keeps; one whose turn ended without it is gone. While it does, its dot says what. What an agent
    aborted without a word left - its partial answer, the stop button, calls without a result - goes
    or is shown as abandoned once the channel says it was removed. A chain between agents cut off at
    its limit says so, naming the agents that were not asked, and that a message of a user starts a
    new chain. A user someone nudges is shown that they are asked to respond.
*   **Who joined or left** is a line of its own where it happened, live and from the history alike:
    "@alice has added @bob to the channel", "@bob has joined the channel", "@bob was removed from
    the channel" - by whose act it was, named as they were then (issue 408). An agent's history
    panel shows it as well.
*   **A plan** is shown by its call, its steps numbered. While an agent works through it, each step
    is a block of its own, headed with the step, that holds what the step said; the step a turn is
    in is open while the turn is under way, every other one folded unless the user opened or closed
    it. A step's answer under way streams in its block while nothing follows the block; once
    anything else stands below it, the answer streams below that, where it will stand when
    complete. Internal messages before any step's mark are headed "the plan". None of
    them is offered for another answer.
*   **Changing one's own messages.** A participant's own message can be edited in place or taken
    back, which asks once more. The editor shows what the message carries, each to remove, and
    attaches more as the message field does, classified as the message; a modification names the
    attachments as they are to stand. A modified message shows "edited"; one taken back is a
    placeholder naming who took it back. Both are applied to the message in place as the server
    reads them. A modification keeps the message's classification.
*   **What the channel did not let in.** What the user sends is shown as they sent it, and the
    channel decides whether it enters. A message refused is marked so, with why; a refused change of
    a message is undone by reading the message back (`GET …/messages/{mid}`).
*   **Asking for another answer.** An agent's last message has a retry icon while nothing is being
    generated and its participant's `can_retry` allows it - never an external agent's, which never
    answers again. So has an answer under way, once its turn is known, and the notice of a turn
    given up because it failed, the channel was lowered or it reached the limit of its model calls
    (`mayBeAnsweredAgain`), while it is the last thing in the chat.
    The agent decides: accepted, its answer is taken back and the new one follows - what a turn
    under way said stays, since what was said is not unsaid; rejected, a notice says why.
*   **Approvals.** A call that needs approval is shown to everybody, with approve and deny only for
    who may approve the agent's calls (`can_approve`); everybody else is told it waits for whoever
    is responsible for the agent. It disappears only when the approver ends it - decided, timed out
    or withdrawn. The pending approvals come with the snapshot, and a request asked anew after a
    restart is shown once. Whether this user answered is known only to this page. Unless a rule of
    the place asked for it, a card offers to add a rule derived from the call with the answer
    ("and from now on") - a command line by how many of the words it was proposed with, which words
    a line has being the server's to read; a value too long for a pattern as it is. Where the server
    proposes no rule, none is offered. A request a rule of this user's answered says so.
*   **Questions** an agent's call waits for are shown to everybody the server sends them to, above
    the approvals, with the controls only for the users they are put to. A question may be skipped,
    the whole of them declined; a question's text keeps its line breaks, so that a command of
    several lines reads as written. Sensitive questions mask the input and say that the answers go
    to the tool alone; the others that they go to the agent, and that writing a message instead
    declines them.
*   **Brave mode** adds a rule that allows every tool call in the session for an hour; while a rule
    of this user's allows everything in the channel, a bar says so, until when, and stops it.
*   **Stop.** The stop button ends the current turn of every agent; the stop icon on an answer under
    way ends that agent's turn alone; a tool call that runs has a stop icon that interrupts that
    call alone, and the agent goes on. A command a call handed back as still running can be ended by
    whoever may change the workspace's files.
*   **Input.** A line starting with `/` is sent as a command, `//` as a literal slash. Typing `@`
    completes the participants' handles, `/` the commands the user may see - the channel's, and the
    agents' with each handle that carries one out. An unsent draft survives a forced re-login. A
    message can be sent while an agent answers: the agent hears it once its turn ends and answers in
    a turn of its own, if asked to. `Enter` sends, `Shift+Enter` breaks the line. A user's message
    is shown with its line breaks as typed, an agent's as Markdown.
*   **Attachments.** The paper clip beside the field, files dropped onto the chat and files pasted
    into the field attach them to the message being written. Each is uploaded at once, classified as
    the release picked as it is attached - a release picked later changes the text's label alone -,
    and shown before sending with its name, size, a preview of an image and a remove; one refused
    says why and has to be removed. A message of attachments alone is sent; while one still uploads,
    nothing is. A command leaves what is attached for the next message, and what is attached while a
    message is sent stays for the next one. What is attached and not sent is not kept: another
    channel or a reload lets go of it, and the server lets its upload expire.
*   **What a message carries** is shown at it: an image as a preview linked to its full size, any
    other file with name, type, size and a download, each through the message (see
    [REST-SERVER.md](REST-SERVER.md#endpoints)); one classified otherwise than its message says how.
    What a tool's result showed its model again is shown in the call's frame the same way, through
    the message that carries the result. An image that cannot be fetched is fetched again a few
    times - one's own message is shown as it is sent or changed, before the channel has it so in
    its history - and then named as a file.

### Classification

The channel's classification is shown above the participant list. Next to the input, a user can
release the next message to a lower level or further compartments; it offers only what may enter the
channel, the server decides, and a release is for one message. Messages whose classification
differs from the channel's carry a badge.

Who may reclassify - the channel's owner the channel, the author or the owner a message - does so
with one dialog, which says what a change releases before it is made. A message takes its new
classification from the channel's stream; a reclassified channel fetches its messages again, which
lowering it may have changed.

### What the channel is for

The panel beside the chat is the channel panel. At its top stand the channel's summary and its
briefing, rendered as Markdown; who may change the channel changes both, and the dialog says who
reads them. A session's description is opened from its menu in the sidebar, read-only for whoever
may not change the session, and says that everybody in the session and every agent in its channels
reads it, external ones too. The session is fetched before it is changed: the list holds no
briefing, and saving without it would remove it.

### Language

The language the channel is held in is shown below its classification - marked as the session's
where it has none of its own; which one holds, the server says (`effective_language`), and the
channel is fetched again when its own or its session's changes. Who may change the channel changes
its own or takes it back; the dialog says that nothing said before is translated.

### Approval rules

The channel's approval rules are counted below the language and changed in the same dialog as the
session's. A rule names a tool - `*` for every one - and optionally per argument a pattern, the
words a command line's commands begin with, or the exact value, and asks or denies; the dialog
offers nothing that allows, since a place cannot. The rules are saved as a whole. An approval asked
for by such a rule says so on its card.

### The channel's files

The channel panel's "Files" shows the session's workspace as the channel names it - where the
channel uses it and the user may read it - and its files, by the workspace; or that no workspace is
used here, without why. The files are loaded again when a tool call of the channel ended, when the
user ended a command, and when the channel's switch to use the workspace changes.

### Participants, moderator and notes

Under each agent, a gauge shows how full its context is: the tokens its last call measured, or a
compaction estimated (`context_tokens`), against the size of its context, coloured as it fills;
without a known size, the tokens alone. Its hover card keeps two things apart: the context, and the
usage of all of the agent's calls in the channel as totals - which grow much faster than the
context, since every call is given the whole context again. Above the list, "Total tokens" adds up
the usage of the agents shown. An agent's numbers are updated by each of its calls and fetched again
when its turn ends, it carried out a command - `/compact` ends without the end of a turn - or it
compacted its context, which also counts calls whose messages were not published. An external agent
reports none.

Each agent has a dot for where it stands, starting from the snapshot and following its state
changes: idle, working, carrying out a command, busy with something of its own - what, as its
activity says -, waiting for an approval, sleeping, or nothing running. Users and approvers have
none.

The participant list is sorted by name and uses the participants' handles for `@` completion and for
highlighting mentions; participants that cannot be mentioned, such as a deleted user, have none. The
completion offers `@all` and `@channel` as well, and both are highlighted in a message that
mentions anyone. Who
answers - the channel's responder policy - is shown above it as its moderator, and for
`mentions_only_policy` the input says that agents answer only when mentioned. `decision_policy`
is explained, but not offered for a new channel: an operator configures it alone.

What can be done with a participant is in its menu, each entry only where the participant offers it:
its profile, its history, its private notes, its tools, removing it from the channel (where
`can_change` is set, which also offers adding a participant of the session with how much of the
history they may see - and, for whoever may change the session, someone found by the beginning of
their name who is not in it yet, marked so and taken into it as well; whoever cannot be added is
shown with why; or a new agent of a personality, made and then taken in - the user's own, or for
this session alone, as they choose or as a personality of the session or the channel requires, and
deleted again if it cannot be taken in),
and deleting the agent - which asks once more, since the agent leaves every channel it
takes part in, not only this one, until it is restored.

*   **The profile** shows name and handle; of an agent its personality, whether it runs here or
    elsewhere - where what it is given leaves the system - and who owns it; and its introduction, a
    user's as "in their own words".
*   **The tools** (`can_read_tools`) are what the agent can call in this channel now, by tool set,
    each set with why it is unavailable where it is, each tool marked where a call needs approval,
    is shown to its owner only, or has its results classified.
*   **The private notes** (`can_read_context`) are what the agent noted for itself in every channel
    of the session the user may read, this channel first, fetched again whenever the agent's state
    in the session changes. Who may read an agent's context sees them there anyway; the panel shows
    them current and on their own.

Below the participants, "Former participants" lists who took part and left, with history and private
notes where `can_read_context`, so that what a removed agent did - the turn it gave up when removed
included - can still be read. A "Notes" tab shows the notes the channel's agents published.

### What an agent saw

Stored messages of agents whose context the user may read carry an eye icon, which opens the
context panel: the messages of the context and the prompt parts where they were given, the
knowledge read, and - collapsed - the tools offered by tool set. Every item shows how it was
classified - "counts as the channel" where that is not known - and what set the message's label is
marked. A channel of another session, which an agent's memory came from, is not named. The panel
shows internal messages, tool results and the reasoning the model was given, which the chat folds,
marked "owner only" where it is the owner's. For an answer of an external agent it shows the
messages the remote agent was given in the call that produced it, and nothing else. The panel's
wording holds for both kinds of agent, since the REST API does not tell them apart. A context never
changes, so what was fetched once is kept. See
[AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw).

The history panel shows the channel's history as the agent saw it (issue 417): what was offered to
it and what it said itself, in full, which may differ from what the channel shows. What its
perceive processors did not pass on is greyed out and marked "not passed on"; a message whose
context is recorded opens it in the context panel. Where a turn was given up, a command carried
out or the context compacted, it is marked where it happened, with why, on whose word, and when;
what replaced the older messages in a compaction follows its mark. It is read in pages like the
chat; when the history grew, the pages shown are read again from the end, back to the oldest shown,
so that the messages stand as they do now.
