# Tools and Commands

Agents act through tools; people address the system through user commands. Both are contributed by
components and built the same way: a `…Provider` facet, and something that receives the ready
providers and checks permissions. This document describes tools, how their calls are run and
approved, user commands, and lists the built-in ones. How components are built is in
[COMPONENTS.md](COMPONENTS.md); how far a tool call may travel is in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md).

## Tools

A component offers tools by implementing `ToolProvider`; one component may expose several `Tool`s.
The trait has these methods:

*   `description()` - what its tools are for, written for a model: the description of its tool set.
*   `tools()` - the tools the component offers.
*   `unavailable()` - why it gives no tools now, in general terms and never with the error; by
    default `None`, available.
*   `execute(name, params, answers, turn)` - runs one of them, with the `answers` to its questions
    if it asked any.
*   `describe(name, params)` - the text approvers are shown for a call; by default the tool's
    description.
*   `matcher(name, params)` - what other calls are like this one (issue 13): the matcher a rule
    derived from the call is offered with (`ApprovalRequest::suggested`); by default the calls
    exactly like it (`ToolMatcher::exact`). A tool whose argument is a language says better what
    makes two calls alike.
*   `questions(name, params)` - what the call asks a person before it runs (issue 165), by default
    nothing. The answers are handed to `execute`; the model learns of them only what the tool puts
    into its result. The questions, their limits and the answers are `caelumora_model::question`:
    up to five questions, each answered with text or by choosing among two to four choices -
    several with `multi_select` - or by writing another answer, and put to those the tool names
    (`ask`). A tool may mark them `sensitive`, for answers the model must not see. How they are
    asked is described under [Questions](#questions).

A `Tool` declares its `name`, `description` and `parameters`, whether it `requires_approval`, its
`visibility`, optionally the `classification` of its results, and its `presentation`: how frontends
show its calls (`ToolPresentation`, issue 165), so that they need not recognise a tool by its name.
The presentation is display only - what a call does, and whether it waits for anyone, does not
depend on it - and is stamped on every call (`ToolCall::presentation`) as the visibility is.
Approval and classification are decided per tool by its provider, not by the personality; `mcp_tool`
takes them from its parameters `requires_approval` - with what the server says of each tool, if so
configured - and `classification`.

`execute` answers with a `ToolOutput`: the tool's value and, if the tool classifies it - by its
input, by what it read, by the source it drew on - its classification. A tool that does not classify
returns a plain value (`Ok(value.into())`).

### Tool sets

The tools of one provider are a *tool set* (`ToolSet`, issue 194), named after its component -
unique within the agent - with the provider's description and, if it is unavailable, why. A model's
backend is given the tools natively, without the sets; the sets are kept in the record of what the
model was offered and shown in the web UI. The description is the provider's own: built-in
components word it for their tools, and only a generic provider such as `mcp_tool` takes one from
its configuration.

The model needs no prompt to learn of its tools, but a source that is unavailable leaves no trace in
what it is offered (issue 32). The optional component `tool_sets_prompt_provider` names the
unavailable sets of the tool sets the turn began with (`AgentWork::tool_sets`) - each with its
description and why, in general terms - and tells the model to say they are unavailable rather than
that it cannot do what they do. While every set is available it contributes nothing. A personality
without it tells its model nothing of missing tools.

A set may say where the arguments of its calls go (`ToolSet::destination`, issue 180): the
`GuardedToolExecutor` offers it as unavailable - "its remote side is not cleared for this channel" -
where the channel may not flow there, and answers a call of a tool the turn was not offered as
unknown (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-leaves-through-tools)).

### Tool visibility

Every tool declares a `ToolVisibility` (issues 54, 133): whether the call itself is published to the
channel, and whether its content - arguments and result - is, or only the agent's owner sees it.

*   `SHOWN`, the default: others see the call in full.
*   `CALL_ONLY`: others see that the tool was called, but not with what.
*   `HIDDEN`: the whole call stays with the agent.

How these flags are stamped on calls, how published messages are redacted, and what the model is
told about results it must not quote is described in [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

### Classified results

How sensitive a result is, is its classification (issue 133). The `GuardedToolExecutor` gives every
result one: as the tool classified it, else as the tool is configured (`Tool::classification`, as
the turn began with its tools), else as the channel. A result that may not flow into the channel
(`ActionType::Publish`, in the agent's name) never reaches the model: it is replaced by the status
`withheld` and a message that names the classification and asks the model not to obtain it another
way. The value is not kept anywhere. Results that did not come from the tool - refused, denied,
timed out - are the runtime's fixed texts and classified as the turn's label; a failed one, whose
error may come from the tool, is the channel's. Built-in tools whose answer is derived from the
arguments alone classify it as the turn's label too (issue 168): the model sees every result, and
what it has seen classifies what the turn writes (see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-a-turn-writes-as)). See
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#tool-results).

### Running tool calls

How an agent runs its tool calls is a component too (issue 127): the `ToolExecutor`. An agent has
exactly one - its personality's, the one `AppConfig::component_defaults` names, or else the default
`default_tool_executor` (see [COMPONENTS.md](COMPONENTS.md#facets-of-which-there-is-exactly-one)).
So another way of running tools - in parallel, with retries, cached - can be tried by configuration.

The executor decides *how* a call runs, never *whether*, and not what the model is told. It answers
with what the tool gave (`ToolOutput`) or a `ToolError` - `UnknownTool` if no tool has the name. The
agent holds it behind the runtime's `GuardedToolExecutor`, which checks before it delegates:

*   that the caller is an agent - only agents call tools;
*   that the policy permits the call (`ActionType::CallTool`);

and makes the result for the model afterwards: it times the call, turns an error into a status and a
next step (issue 15), classifies the result and withholds what may not flow into the channel (issue
133), and marks what only the owner sees (`successful_result`). The code that decides about
permissions and about what the model sees is thus never the choice of whoever may write a
personality.

A call is not waited for longer than its turn lasts (issue 161). The guard runs it next to the
turn's `CancellationToken`, which a cancel and a stop cancel; then the executor's future - and with
it the tool's - is dropped wherever it stands, the call is answered as `interrupted`, and the agent
starts no further call of the step. A tool that is done when the token is cancelled keeps its
result. Dropping a tool does not undo what it did ([20](../issues/20-transactional-effects.md)):
the call keeps when it started, so that it counts as having run - a turn given up keeps the call
and its result where they were said (issue 258) - and the model is told to check its effect
before calling it again.

Dropping a future ends it: none of the tool's code runs after the `await` it stood at, and what the
future holds - timers, sockets, open requests - is released. That is all the stopping a tool gets;
it does not see the token, and there is no way yet for one to clean up otherwise. A tool therefore:

*   expects to be dropped at any `await`, and never leaves the turn half changed across one;
*   binds whatever it starts to its own future, since that is all that is dropped: a task it spawns
    is aborted on drop (e.g. through `tokio_util::task::AbortOnDropHandle`; a plain `JoinHandle`
    only detaches it), a child process is started with `kill_on_drop(true)`, and work on a thread
    of its own (`spawn_blocking`, `std::thread`) cannot be interrupted at all - the one exception
    is `terminal_tool`, whose commands live with the storage
    ([The workspace](#the-workspace));
*   does not block: code that never reaches an `await` - a long computation, `std::thread::sleep` -
    cannot be interrupted, and holds up the agent's whole task, which even the abort after
    `STOP_GRACE` does not end.

What happened outside the process - a request the remote side already received - is not undone,
and whether the remote side stops depends on it (for MCP servers see below).

The default executor has the `ToolProvider`s of the agent's components injected, with their
component ids. It runs a call on the provider that has the tool - no more - and gives the agent's
tools as one tool set per provider (`ToolExecutor::tool_sets`), asked of the providers each time.

### Two tools of the same name

Two components providing a tool of the same name are a configuration error that keeps the agent from
starting, naming both components. Resolved by order, one of them would be silently unreachable
(issue 37). User commands follow the same rule.

### MCP servers

An `mcp_tool` component exposes the tools of a remote MCP (Model Context Protocol) server, reached
over the Streamable HTTP transport:

```json
{ "id": "github-mcp", "kind": "mcp_tool", "params": { "url": "https://mcp.example.com/mcp", "auth": { "type": "bearer", "secret": "GITHUB_MCP_TOKEN" } } }
```

*   **Several servers.** Each server is an `mcp_tool` instance of its own. Its tools are named
    `"{component_id}__{remote_tool_name}"`, so that servers cannot collide.
*   **Discovery.** The tool list is discovered once, when the component is built, and cached. A
    server that is unreachable then does not fail the agent; it starts with no tools from that
    server, and a warning is logged with the error. Its tool set is unavailable, with why in
    general terms - "its server cannot be reached", never the error, which may name hosts - so
    that `tool_sets_prompt_provider` and the web UI can tell (issue 32). Discovering again is
    issue 126.
*   **Description.** The tool set is described by the parameter `description`, else by what the
    server says of itself in the `instructions` of its `initialize` result, else as "The tools of
    the MCP server '<component id>'" (issue 194).
*   **Credentials.** `auth.secret` names a secret, resolved through the secret chain (see
    [SECURITY.md](SECURITY.md)); it is never a literal credential.
*   **Parameters.** `url` (required), `auth` (optional; only `bearer` so far), `timeout_secs`
    (default 10, for discovery and for every call), `requires_approval` (`true`, the default,
    `false` or `"unless_read_only"`; see [What needs approval](#what-needs-approval)),
    `classification` and `description`.
*   **Calls.** Each call opens a connection of its own. The remote structured content is the result
    if there is one, else the joined text; a remote error is a failed call.
*   **Interrupted calls.** An interrupted call (issue 161) drops its connection. rmcp then closes
    it in the background: it aborts the HTTP request under way and ends the MCP session
    (`DELETE`, if the server keeps sessions). No `notifications/cancelled` is sent - rmcp does so
    only for its own timeout - but as each call has its session of its own, the session ending says
    the same. Whether the remote tool stops is up to the server.

## Approvals

Some tool calls need a person's approval before they run. The process is asynchronous: the agent
suspends its turn while it waits, and nothing blocks.

1.  **Interruption.** When the model's answer contains tool calls that require approval, the
    `AgentActor` registers them as waiting for it, runs whatever needs no approval, and moves to
    `WaitingForUser`. Its state, the waiting calls included, is stored after every step.
2.  **Request.** The agent emits `ApprovalRequested`, carrying the call's `ApprovalId`. The
    moderator routes it to the channel's `ApproverActor` only. The approver passes it on unchanged
    to the users (`UserActor`s), whose frontends show it - to everybody, so that all see what the
    agent waits for; only who may approve is asked (see below). A request nobody taking part may
    approve is denied at once instead.
3.  **Answers.** A user approves or denies over REST (`POST …/approvals`) or in the TUI.
    `UserActor::send_approval` sends `ApprovalGranted` or `ApprovalDenied` with the same id, which
    the moderator again routes to the approver only. A rule of the user's may answer for them,
    the same way (see [Rules of a person](#rules-of-a-person)).
4.  **Decision.** The approver collects the answers of those who may approve and decides by its
    `ApprovalStrategy` (`Any`, `Majority`, `All`, `DenyOnFirst`), counted among them alone. It
    decides again when users join or leave. A request nobody decides times out after ten minutes.
    The approver sends `ApprovalGranted`, `ApprovalDenied` or `ApprovalTimedOut` to the agent - the
    only way a decision reaches an agent (issue 81) - and to the users: an approval is over only
    once the approver says so, not when one user answered.
5.  **Resumption.** The agent runs the granted calls and continues its turn.

Where a call of the current step stands is its `ToolCallStatus` (issue 165), kept with the turn:
`WaitingApproval` - with the approval's id and what the approvers are shown - then `Ready` once it
may run, and `Finished` with its one result once it ran or was answered without running. How it
ended is not a status of its own but the result's (see [Results](#results)): a denial finishes the
call at once with `denied`, a timeout with `timed_out`.

A turn that ends while it waits - cancelled, given up - withdraws its requests (issue 148): the
agent sends `ApprovalRequestWithdrawn` with the id, which the moderator routes to the approver only.
The approver takes it only from the agent that asked, forgets the request without deciding it, and
passes the withdrawal on to the users, so that nobody is asked any longer. A stopped
agent withdraws nothing - it has left - and the approver answers its pending requests as it leaves
(see [CHANNELS.md](CHANNELS.md)).

The agent's turn is the record of what it waits for (issue 172): the approver holds its requests
only in memory and starts empty after a restart. An agent that resumes a turn with calls still
pending - in `WaitingForUser`, or stored right after asking - asks again, with the same ids
(`TurnState::pending_approvals`). The approver takes a request it does not know up as new, with a
new timeout; one it still holds it only announces again, keeping the answers given so far. Answers
given before a restart are lost with the approver's memory: the users are asked again.

### What needs approval

What the model believes about who said what is no ground for a consequence (issue
[150](../issues/150-impersonation-towards-the-agent.md)): a participant can write another's sender
marker into their own message, and models believe it. A call runs under the agent's permissions
(`ActionType::CallTool`), never under those of whom the model takes for the speaker, so a faked
marker gains the agent nothing it could not do anyway - but it can make the agent do it. What needs
approval follows from that:

*   **A tool that acts outside** needs it. `mcp_tool` requires approval for every tool of its
    server unless its configuration says otherwise: `requires_approval: false` for none, which fits
    a server that only reads, or `"unless_read_only"` for all but the tools the server marks as
    read-only (`readOnlyHint`, issue [246](../issues/246-mcp-tool-hints.md)). A tool without that
    mark counts as one that changes something, as MCP says. The mark is the server's word: it is
    trusted only where the configuration says so, and it relaxes nothing beyond that - `false`
    stays none, `true` stays all. Read-only does not mean that nothing leaves the system: the
    arguments of a search reach the server all the same (issue
    [180](../issues/180-outbound-flow-unchecked.md)).
*   **Releasing** a note, a private note or something remembered to a wider circle needs it
    (`reclassify_…`, issue 133).
*   **What stays in the conversation** needs none: reading (`read_notes`, `read_history`), within
    what the agent may read and its channel may take in; the tools without an effect; and ending
    the turn (`stop`, `bye`, `silence`) - an agent put to sleep on someone's word is woken with
    `/wakeup`. A plan (`create_plan`) is one of them: it says what the agent intends, and every
    step with an effect is approved on its own. Whoever wants to see plans before they run sets a
    rule of a place that asks for `create_plan` (see below); the request shows the plan.
*   **Notes and memory** need none either, although what the agent writes there may rest on a
    faked word and from then on stands in the agent's name: a note before everyone in the channel
    and in what other channels read of it, something remembered in the agent's other channels, seen
    by nobody but the person responsible for it. That is accepted: an approval for every note would
    leave notes unused, a note is shown to every member, and every entry records where it was made
    (see [MEMORY.md](MEMORY.md)).

Whoever approves decides on what the call does, not on the agent's account of why; what was said,
and by whom, the context panel shows.

### Rules of a place

Beside what a tool says of itself, the places an agent works in restrict its calls (issue
[13](../issues/13-approval-modes.md)): the system (`AppConfig::approvals`, see
[CONFIGURATION.md](CONFIGURATION.md)), its session and its channel (`approval_rules` of the
document, set by whoever may change it - `UpdateSession`, `UpdateChannel`). A rule
(`caelumora_model::approval::ApprovalRule`) names the tool as the model calls it, `*` for every
tool, and optionally a regular expression per argument that its whole value must match - a string
as it is, any other value as its compact JSON; it may expire. It decides one of two things:

*   **`deny`** - the call never runs. It is answered `forbidden`, as a refusal of the policy is,
    before anyone is asked, and again if the rule was set while the call waited for its approval.
*   **`ask`** - a person decides this very call, whatever the tool says: the request is *explicit*
    (`ApprovalRequest::explicit`), which the web UI and the TUI show. Rules a person sets for
    themselves will not answer an explicit request.

A place cannot allow: there is nobody at a place who would stand for an approval
(`ApprovalRule::check_restricting` refuses an `allow` and a reach narrower than the place, and so
do the configuration and the controllers). Of the rules of all three places that apply to a call,
the strongest decides - `deny` before `ask` (`ApprovalRule::decide`). The `GuardedToolExecutor`
reads them as they stand at every call, from the controllers of its session and channel; the
approver is not involved. Patterns are matched with the `regex` crate, which has no backtracking,
and are bounded in length and compiled size; a rule whose pattern cannot be matched - one written
into a document by hand - restricts and never allows. A pattern is reliable for arguments with a
structure of their own, not for one that is a language, such as a shell command.

### Rules of a person

A person can give their answer in advance (issue 13): "for my part, `ls` is always fine". Such a
rule has the same shape as a rule of a place, but it allows or denies - asking is what happens
without one - and has a reach of its own: everywhere, one session or one channel. The rules are the
person's, kept in their `UserProfile`, which nobody else reads; others in the same channel may have
other rules.

A rule is not evaluated by the approver, which has no right to approve anything of its own, but by
the person's `UserActor`, when a request reaches it from the approver: if a rule of theirs applies
(`ApprovalRule::decide`), and they may approve the agent's calls in the channel (`may_approve`,
asked first), the actor sends `ApprovalGranted` or `ApprovalDenied` as their click would, and shows
it to them. To the approver it is an answer of theirs like any other; `ApprovalResponse::by_rule`
names the rule, for the record and their own view - only their actor sets it, `send_approval`
clears it. The approver's `ApprovalStrategy` decides between people with opposite rules as between
people who clicked: under `Any` an `allow` wins whoever answered first, under `All` and
`DenyOnFirst` a `deny` does.

*   **Explicit requests** - those a rule of a place asks for - are answered by no rule.
*   **Answers come at once.** Under `Any`, a person's `allow` decides before anyone read the
    request: a rule weighs as much as the button.
*   **The brave mode** is a rule like any other: every tool (`*`, no patterns), allowed in one
    session, for a while. The web UI offers it for an hour and shows it for as long as it lasts.
*   **A rule is derived from a call**: the approval card offers "and from now on", with the
    matcher the call's tool suggests, a reach and an expiry. The person answers the call first; the
    rule is added after (`POST /profile/approval-rules`).
*   Rules are added and taken back through the user's controller, which stores them first and
    announces the profile (`UserProfileUpdated`), and drops those that expired with every change. A
    person keeps at most `MAX_PERSONAL_RULES`. A rule is bound to no agent: `may_approve` bounds it.

### Who may approve

Who may approve a call is the `SecurityManager`'s to decide, and the approver enforces it (issue
14): the action `ApproveToolCall` carries the attributes of the agent - loaded, never taken from the
request, which the agent makes itself - and of the channel. The ownership policy grants it the
agent's owner and contributors, who are responsible for it also in someone else's channel; for an
agent of the system, the channel's owner.

*   An answer counts only if its sender may approve: the approver asks with the context the answer
    came with (`Enforcement::require`) and leaves out any other, logged and without telling the
    sender - only who may approve is offered the buttons, so such an answer comes from a client
    going round what it was shown.
*   The strategies count only those taking part who may approve, now: the approver asks
    `may_approve` for every user of the channel, in the context of their own `UserActor` - the one
    their answers come with. An answer of someone who left or may no longer approve does not count,
    either way. With nobody left who may approve, the request is denied.
*   The views offer it with `MemberView::may_approve` (`can_approve` of a member), which the web UI
    shows the buttons by.

Messages that arrive while the agent waits for approval are queued and processed in order once it
resumes, so its context stays consistent.

Approval requests are not redacted by confidentiality: the approvers see the arguments of a call
they are asked to decide.

### Checked before anyone is asked

Before a call is registered, the runtime's guard decides how it is admitted
(`GuardedToolExecutor::admit`, issue 155): answered at once, put to a user if its tool requires
approval, or run as it is. The actor only records what the guard decided.

*   **Policy.** A call the `SecurityManager` does not permit (`ActionType::CallTool`) is refused
    before any approval is requested, so users are not asked about calls that would be refused
    anyway. `execute` checks again.
*   **Validity.** A call that cannot run - arguments its tool's `describe` rejects, a tool that does
    not exist - is answered at once instead of being put up for approval (issue 118).

### Results

Every call ends with a result the model can act on (issue 15).

*   A successful call's result is the tool's output.
*   Every other outcome reaches the model as a JSON object `{"status": ..., "message": ...}`, whose
    message says what happened and what to do next. The statuses are `denied` (by a user),
    `forbidden` (by policy - without the reason, which stays in the log), `timed_out` (no approval
    in time), `declined` (a user declined the call's questions), `invalid_arguments`, `failed` (with
    the tool's error), `not_executed` (the tool never ran: its turn ended first, or the server
    restarted before it ran), `interrupted` (the tool ran, but its turn was cancelled or stopped
    before it ended) and `unknown_tool` (with the tools that do exist).
*   The same status is stored as `ToolCallResult::status`, so the UI and an audit trail can tell
    refusals and failures from results.
*   The wording lives in `crate::tool` (`ToolError::to_result`, `denied_result`, `timed_out_result`,
    `declined_result`, `forbidden_result`, `abandoned_result`, `not_executed_result`,
    `interrupted_result`).
*   No call is left without a result. A turn that ends while calls of its step have none - cancelled
    while waiting for an approval, given up - answers them as `not_executed`, saying why the turn
    ended, and says the tool message with the results of those that did run (`ToolCalls::abandon`,
    issues 148, 258).

A call that ran also records when: `ToolCallResult::started_at` and `finished_at`, set by the
executor around the tool's own run (not the checks before it) - `finished_at` is when an
interrupted call was interrupted. They are stored with the history and
published with the call, so that a client can show the duration (issue 100).

## Questions

A tool may ask a person before its call runs (issue 165): `ToolProvider::questions` says what, and
the answers are handed to its `execute`. They are no messages, and the model learns of them only
what the tool puts into its result. The turn waits for them as for an approval, without blocking:
the call is parked, the turn stored, and it goes on with the result once the answers have come.
The built-in tool `ask_user` (`ask_user_tool`) is the simplest such tool: the model writes the
questions and names whom to ask by their handles, and the tool's result is the answers.

1.  **Admission.** `GuardedToolExecutor::admit` asks the tool for the call's questions after
    `describe`, checks them (`Questions::check`: one to five, each with a header of at most twelve
    characters, no choices or two to four with distinct labels, someone asked) and puts them to
    the users of the channel (`ChannelLoop::put`): a handle as participants are mentioned, with or
    without `@`; `Owner` as the agent's owner. A handle that names no user of the channel, or an
    owner who does not take part, makes the call invalid, answered at once. A call whose tool
    requires approval, or that a rule of a place asks for, is put up for approval first
    (`ToolCallStatus::WaitingApproval` keeps its questions); approved, it asks them. Whoever
    approves sees the arguments, never the answers.
2.  **Request.** A call that asks waits in `ToolCallStatus::WaitingUser`, the agent in
    `WaitingForUser`. It sends `QuestionRequested` with the call and its questions, under a
    `QuestionId` of the runtime's - not the model's call id, which some backends repeat. The
    moderator sends it to the users of the channel - to those asked alone if the questions are
    sensitive - never to the approver or other agents. Everybody it reaches sees what the agent
    waits for; only those asked answer.
3.  **Answers.** A user answers over REST (`POST …/questions/{qid}`) or in the TUI - which shows the
    questions as text, asks them one by one after `/answer [id]` and declines them with
    `/decline [id]` - through `UserActor::answer_questions`, which asks `AnswerQuestion` of the
    channel and refuses what cannot count - questions not open or not put to them, responses not as
    the questions ask (`Questions::check_responses`) - before anything is sent. `QuestionAnswered`
    goes to the asking agent alone. The agent takes it only if its sender was asked and the
    questions are still open, and checks the responses again; who answered it takes from the
    envelope, never from what was said. The first answer that counts settles the call.
4.  **Declining.** The user may decline - `QuestionDeclined` - and a message of theirs written
    while questions put to them are open declines them all: their actor sends the declines before
    the message, so that the turn goes on and the message is heard after it. A declined call is
    answered `declined` without running.
5.  **Closing.** Once questions are no longer open - answered, declined, or abandoned as their
    turn ends - the agent sends `QuestionClosed` to those they were shown to, and every frontend
    closes its form. A cancel or a stop ends the waiting as for an approval.

The turn is the record of what it waits for (`TurnState::pending_questions`): a turn resumed after
a restart asks again, with the same ids, and the snapshot of a channel shows the open questions.
While a call waits to run, its answers are not stored (`ToolCallStatus::Ready` skips them): a
call ready to run when its turn is resumed is not run but answered as not executed, so nothing is
lost.

Once the call has finished - run, declined, or abandoned - its result keeps the record of the
questions and their answers (`ToolCallResult::answers`, issue 165): who answered or declined, and
what, whatever the tool put into `content`. The record goes with the result into the transcript
and the history, and is published as `content` is - left out where the content is the owner's. It
is never given to a model: a backend gives the result without it
(`ToolCallResult::for_the_model`).

### Sensitive questions

A tool may ask for what the model must not see - a one-time code, an account to pay to - and marks
such questions `sensitive`. Then:

*   they are shown to those asked alone: the request, their closing and the snapshot entry reach
    nobody else;
*   their answers are not stored and not logged: the result keeps no record of them, not even who
    declined (`Answers::record`), `Answers` and `Response` show no content in `Debug`, and a
    user's own actor does not echo the answers to their other frontends;
*   whom they are put to is the tool's to say, not the model's (e.g. `Addressee::Owner`), so that
    the model cannot choose who is asked for a password;
*   the frontend masks the input and says that the answer goes to the tool alone - a mark the tool
    sets, which a model cannot set on the questions of `ask_user`;
*   the tool does not repeat the answers in its result - its own responsibility;
*   a transport that keeps envelopes ([122](../issues/122-several-instances.md)) must not keep
    them.

## User commands

A line that starts with `/` addresses the system rather than the agents: `/help`, `/wakeup [agent]`
([28](../issues/28-user-commands.md)). A command is not a message: it never enters the history and
never reaches an agent's context.

Commands are for people; agents act through tools. A command is always run on behalf of a particular
person, so it is run by that person's actor, `UserActor::execute_command(name, args)`, as messages
and approvals are. REST reaches it the same way (`user_actor_of`, `403` for non-participants). Only
`UserActor` runs commands. What a command does to the channel still goes through the channel's own,
permission-checked operations (`ChannelLoop::wakeup`, `wakeup_actor`).

### Recognised by the frontend

The client decides whether a line is a command and sends it to `POST .../commands` (`{name, args}`)
instead of `POST .../messages`; the message API stays plain text. The rule is
`caelumora_model::command::parse_input`, mirrored in the web UI's `command.ts`:

*   a leading `/` starts a command, whose name runs to the first whitespace;
*   the rest of the line is handed to the command unparsed;
*   `//` sends a literal `/`.

Only this lexical rule is repeated per frontend. What a command means, who may run it and what it
answers is decided in one place.

### Commands are components

A `CommandProvider` (`caelumora_runtime::command`) contributes commands, as a `ToolProvider`
contributes tools, registered in the `ComponentRegistry`. They come from two places:

*   `AppConfig::commands`, the global set - by default `/help` and `/wakeup`; an empty list switches
    them off (see [CONFIGURATION.md](CONFIGURATION.md));
*   `SessionTemplate::commands`, for commands that belong to what a session is for. They are copied
    into the `Session`, so a running session keeps its set.

`RuntimeManager::new_command_executor` builds both once per session and hands the providers to a
`CommandExecutor` shared by the session's channels. It is the counterpart of an agent's
`ToolExecutor`, which likewise receives ready providers rather than a configuration. Two components
providing the same name are a configuration error that keeps the session from starting.

The two differ in one respect, deliberately (issue 127): the `CommandExecutor` is a runtime type,
not a component, and checks permissions itself. How a command runs is the command's own business,
and there is nothing about running commands to try out.

### Seeing and running

Seeing and running a command are separate permissions:

*   `ActionType::ShowCommand(name)` decides whether a command appears in `/help` and in completion
    (`GET .../commands`).
*   `ActionType::ExecuteCommand(name)` decides whether it runs.

A command the user may not see is answered exactly like an unknown one, so hidden commands cannot be
found by trying names. Whatever the command does is checked on its own as well: `/wakeup` asks for
`WakeChannel`.

### Private or channel-wide answers

A command answers the issuer alone (`/help`) or the whole channel (`/wakeup`).

*   A channel-wide answer is announced by the issuer's `UserActor` as `ActorEvent::CommandExecuted`.
    Every moderator forwards it to the users only; it is shown but never stored.
*   `UserActor::execute_command` returns the answer in both cases, because the channel never routes
    an envelope back to its sender.
*   The REST response carries the answer only when it is private, since the web UI receives the
    announcement on its stream.

## Built-in tools and commands

### Tools

| Tool | Component | Parameters | Approval | Visibility | What it does |
|---|---|---|---|---|---|
| `echo` | `echo_tool` | `message` | no | `SHOWN` | Returns its arguments unchanged. |
| `sleep` | `sleep_tool` | `seconds` (optional, at most 60) | as configured, no by default | `SHOWN` | Waits, then says so; for testing a cancel or a stop while a tool runs. |
| `calculate` | `calculator_tool` | `expression` | no | `SHOWN` | Evaluates a mathematical expression. |
| `stop` | `stop_tool` | - | no | `SHOWN` | Ends the turn: the agent has finished its task and goes to sleep. |
| `bye` | `stop_tool` | - | no | `SHOWN` | Ends the turn: the agent says good-bye and goes to sleep. |
| `silence` | `stop_tool` | - | no | `SHOWN` | Ends the turn without an answer; the agent stays idle. |
| `ask_user` | `ask_user_tool` | `questions` (1-5, each with `header`, `question`, optional `choices` and `multi_select`), `ask` (handles, at least one) | no | `SHOWN` | Asks the users named and waits for their answers, which are its result (see [Questions](#questions)). |
| `create_plan` | `create_plan_tool` | `summary`, `steps` (each with `description`) | no | `SHOWN` | Submits a multi-step plan; a plan in progress is extended, not replaced. Presented as `plan`: the summary and the numbered steps. |
| `{id}__{name}` | `mcp_tool` | as the server declares | per `requires_approval`, yes by default; with `"unless_read_only"` not for a tool the server marks read-only | `SHOWN` | A tool of a remote MCP server. |
| `note` | `notes` | `text` | no | `SHOWN` | Notes something in the channel, visible to every participant. |
| `revoke_note` | `notes` | `id` | no | `SHOWN` | Revokes one of the agent's own notes. |
| `read_notes` | `notes` | `channel` (optional) | no | `SHOWN` | Reads the notes of another channel, or of every readable channel. |
| `reclassify_note` | `notes` | `id`, `level`, `compartments` (optional) | yes | `SHOWN` | Reclassifies one of the agent's notes, never above the channel (issue 133). |
| `note_private` | `private_notes` | `text` | no | `HIDDEN` | Notes something only the agent (and its owner) sees. |
| `revoke_private_note` | `private_notes` | `id` | no | `HIDDEN` | Revokes one of the agent's private notes. |
| `reclassify_private_note` | `private_notes` | `id`, `level`, `compartments` (optional) | yes | `HIDDEN` | Reclassifies one of the agent's private notes, never above the channel (issue 133). |
| `remember` | `agent_memory` | `text` | no | `HIDDEN` | Remembers something across channels and sessions. |
| `forget` | `agent_memory` | `id` | no | `HIDDEN` | Forgets something remembered. |
| `reclassify_memory` | `agent_memory` | `id`, `level`, `compartments` (optional) | yes | `HIDDEN` | Reclassifies something remembered that may be recalled here, never above the channel (issue 133). |
| `run_command` | `terminal_tool` | `command`, `wait_secs` (optional, at most `max_wait_secs`) | as configured, yes by default | `SHOWN` | Runs a shell command in the session's workspace and follows it; answers with its output and exit code, or with its id while it runs (see [The workspace](#the-workspace)). |
| `command_output` | `terminal_tool` | `id`, `offset` (optional), `wait_secs` (optional) | no | `SHOWN` | What a command started in the channel printed since `offset`, once it ended or the wait is over. |
| `kill_command` | `terminal_tool` | `id` | no | `SHOWN` | Ends a command started in the channel, and what it started. |
| `list_files` | `terminal_tool` | `path` (optional, the top) | as configured; not under `"unless_read_only"` | `SHOWN` | Lists a directory of the workspace (see [The workspace](#the-workspace)). |
| `read_file` | `terminal_tool` | `path`, `start_line`, `line_count` (optional) | as configured; not under `"unless_read_only"` | `SHOWN` | Reads a text file of the workspace, or the lines asked for. |
| `grep_files` | `terminal_tool` | `pattern`, `path` (optional), `max_matches` (optional, 100, at most 1000) | as configured; not under `"unless_read_only"` | `SHOWN` | Searches the text files of the workspace for a regular expression. |
| `write_file` | `terminal_tool` | `path`, `content` | as configured, yes by default | `SHOWN` | Writes a text file of the workspace, its directories made. |
| `replace_in_file` | `terminal_tool` | `path`, `old_text`, `new_text`, `replace_all` (optional) | as configured, yes by default | `SHOWN` | Replaces text that occurs once - or every occurrence - in a text file of the workspace. |
| `delete_file` | `terminal_tool` | `path` | as configured, yes by default | `SHOWN` | Deletes a file or an empty directory of the workspace. |
| `read_history` | `channel_history` | `channel`, `last` (optional, default 20, at most 100), `query` (optional) | no | `SHOWN` | Reads the latest messages of another channel, or those containing a text. |

`stop`, `bye` and `silence` end the turn with the current tool step (`Turn::request_state`). Notes,
private notes and agent memory are described in [MEMORY.md](MEMORY.md).

### The workspace

`terminal_tool` (issue [251](../issues/251-open-terminal.md)) gives an agent its session's
workspace, where its channel may use it ([CHANNELS.md](CHANNELS.md#its-workspace)): its files, and
a shell in it where its storage gives one. The workspace is reached through the turn's views on
every call, before anything is sent, and a call where there is none - or a command where its
storage has no shell - fails and says so. `workspace_prompt_provider` tells the model beforehand
what it has. Which calls need approval is configured as for `mcp_tool` (`requires_approval`,
`ApprovalMode`): all - the default -, none, or `"unless_read_only"`, all but those that only read
files.

**Files.** `list_files`, `read_file` and `grep_files` read; `write_file`, `replace_in_file` and
`delete_file` change - over the workspace's `Terminal`, whatever its storage, a shell not needed.
A path is relative to the workspace's top and made a `WorkspacePath` first: one that leaves the
workspace is the model's to correct, as is a file that is not there or not text. A file is read
whole, up to 4 MiB, as text (UTF-8); `read_file` gives the lines asked for, at most
`max_output_bytes` of them, and says where to read on; `grep_files` walks the directory asked for,
at most 2000 files, skipping what is no text. A replacement must find its text exactly once, unless
every occurrence is to be replaced; it reads, replaces and writes, so what changed the file in
between is overwritten. What was read is recorded as the turn's knowledge
(`KnowledgeSource::Workspace`, see [MEMORY.md](MEMORY.md#recording-what-was-read)), a file's
content by its digest. The approver reads a change whole - what is written, what is replaced by
what -, and a rule derived from an approved change covers the file, whatever is written into it
(`ToolProvider::matcher`, the path alone).

**Commands.** A command runs in the workspace's directory, every one anew - a `cd` does not carry
over - and is seen only from the channel it was started in.

A call follows its command for `wait_secs`; one still running then is handed back with its id, and
the model follows it with `command_output` - handing back the `next_offset` of the last result - or
ends it with `kill_command`. Its result is a JSON object `{ id, status, output, next_offset,
truncated, exit_code? }`, `status` one of `running`, `completed`, `killed` and `failed`, `output`
cut from the front to `max_output_bytes`; it is classified as the workspace. The approver reads
the command whole ("Run in the workspace:" and the command on a line of its own); following and
ending a command need no approval, since only a command that was approved is followed or ended.
Its calls are presented as `shell` (`ToolPresentation::Shell`): the web UI shows the command as a
line of code and the output as a terminal shows it.

A call dropped while it waits for its command - the turn cancelled or stopped - kills the command:
the one thing a tool does after its future is gone, in a task of its own that is bounded to five
seconds and logged, and that the drop does not await. It is a deliberate exception to "a tool binds
whatever it starts to its own future": a command lives with the storage, so that one handed back
as running outlives the call, and one left running by a dropped call would go on writing into the
workspace after the turn that started it. A command handed back as running is the model's to end;
nothing bounds how long it runs yet (issue [254](../issues/254-commands-run-unbounded.md)).

### Commands

| Command | Component | Arguments | Answer | What it does |
|---|---|---|---|---|
| `/help` | `help_command` | `[command]` | private | Lists the commands the user may use, or describes one. |
| `/wakeup` | `wakeup_command` | `[agent]` | channel-wide | Wakes the sleeping agents of the channel, or the one named (`@handle` or `handle`). |
