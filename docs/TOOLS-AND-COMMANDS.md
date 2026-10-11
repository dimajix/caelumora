# Tools and Commands

Agents act through tools; people address the system through user commands. Both are contributed by
components and built the same way: a `…Provider` facet, and something that receives the ready
providers and checks permissions. This document describes tools, how their calls are run and
approved, user commands, and lists the built-in ones. How components are built is in
[COMPONENTS.md](COMPONENTS.md); how far a tool call may travel is in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md).

## Tools

A component offers tools by implementing `ToolProvider`; one component may expose several `Tool`s.
Besides running a call, a provider says what its tools are for, why it gives none now (in general
terms, never with the error), what approvers are shown for a call (`describe`), which calls are
like a given one (`matcher`, for a rule derived from it), and what a call asks a person before it
runs (`questions`). A tool whose argument is a language says better than the default what makes two
calls alike.

A `Tool` declares its name, description and parameters, whether it requires approval, its
visibility, optionally the classification of its results, and its presentation: how frontends show
its calls, so that they need not recognise a tool by its name. The presentation is display only -
what a call does, and whether it waits for anyone, does not depend on it. Approval and
classification are decided per tool by its provider, not by the personality.

### Tool sets

The tools of one provider are a *tool set* (`ToolSet`), named after its component, with the
provider's description and, if it is unavailable, why. A model's backend is given the tools
natively, without the sets; the sets are kept in the record of what the model was offered and shown
in the web UI. Only a generic provider such as `mcp_tool` takes its description from its
configuration.

The model needs no prompt to learn of its tools, but a source that is unavailable leaves no trace in
what it is offered. The optional component `tool_sets_prompt_provider` names the unavailable sets of
the turn, with why in general terms, and tells the model to say they are unavailable rather than
that it cannot do what they do. A personality without it tells its model nothing of missing tools.

A set may say where the arguments of its calls go (`ToolSet::destination`): where the channel may
not flow there, the set is offered as unavailable, and a call of a tool the turn was not offered is
answered as unknown (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-leaves-through-tools)).

### Tool visibility

Every tool declares a `ToolVisibility`: whether the call is published to the channel, and whether
its arguments and result are, or only the agent's owner sees them.

*   `SHOWN`, the default: others see the call in full.
*   `WITHOUT_RESULT`: others see that the tool was called and with what, but not what it returned.
*   `CALL_ONLY`: others see that the tool was called, but not with what.
*   `HIDDEN`: the whole call stays with the agent.

How published messages are redacted, and what the model is told about results it must not quote, is
described in [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

### Classified results

Every result is classified: as the tool classified it, else as the tool is configured, else as the
channel. A result that may not flow into the channel (`may_publish`, in the agent's name) never
reaches the model: it is replaced by the status `withheld` and a message that names the
classification and asks the model not to obtain it another way. The value is kept nowhere. Results
that did not come from the tool - refused, denied, timed out - are the runtime's fixed texts,
classified as the turn's label; a failed one, whose error may come from the tool, is the channel's.
Built-in tools whose answer is derived from the arguments alone classify it as the turn's label
too. See [CONFIDENTIALITY.md](CONFIDENTIALITY.md#tool-results).

### Running tool calls

An agent's tool calls are run by the runtime's `ToolExecutor`, built with the `ToolProvider`s of
its components. The executor is no component: how a call runs is its tool's business, but whether it
runs and what the model is told is never the choice of whoever may write a personality. Before it
hands a call to its tool, it checks that the caller is an agent - only agents call tools - that the
call was offered to it, and that the place's rules still let it run. Whether the policy permits the
call (`CallTool`) is asked once, as the call is admitted (see [Checked before anyone is
asked](#checked-before-anyone-is-asked)), not again as it runs. Afterwards the executor times the
call, turns an error into a status and a next step, classifies the result, withholds what may not
flow into the channel and marks what only the owner sees.

**Two tools of the same name** are a configuration error that keeps the agent from starting, naming
both components: resolved by order, one of them would be silently unreachable. User commands follow
the same rule.

**A call is not waited for longer than its turn lasts.** A cancel or a stop drops the tool's future
wherever it stands; the call is answered as `interrupted`, and the agent starts no further call of
the step. A tool that is done already keeps its result. Dropping a tool does not undo what it did
([20](../issues/20-transactional-effects.md)): the call counts as having run - a turn given up keeps
it with its result - and the model is told to check its effect before calling it again.

A user may also interrupt one call alone and let the turn go on (see
[CHANNELS.md](CHANNELS.md#cancelling)). A call interrupted while it runs is dropped as above and
answered as `interrupted`, saying who interrupted it and that the turn goes on; one interrupted
before it runs is answered as `not_executed`. The other calls of the step run. A shell command
handed back as running is no call any more; a user ends it as a command (see [The
workspace](#the-workspace)).

Dropping a future is all the stopping a tool gets: none of its code runs after the `await` it stood
at, and what the future holds is released. A tool therefore:

*   expects to be dropped at any `await`, and never leaves the turn half changed across one;
*   binds whatever it starts to its own future: a task it spawns is aborted on drop (a plain
    `JoinHandle` only detaches it), a child process is killed on drop, and work on a thread of its
    own cannot be interrupted at all - the one exception is `terminal_tool`, whose commands live
    with the storage;
*   does not block: code that never reaches an `await` cannot be interrupted, and holds up the
    agent's whole task, which even the abort after the stop's grace does not end.

What happened outside the process - a request the remote side already received - is not undone,
and whether the remote side stops depends on it.

### MCP servers

An `mcp_tool` component exposes the tools of a remote MCP server, reached over the Streamable HTTP
transport:

```json
{ "id": "github-mcp", "kind": "mcp_tool", "params": { "url": "https://mcp.example.com/mcp", "auth": { "type": "bearer", "secret": "GITHUB_MCP_TOKEN" } } }
```

*   **Several servers.** Each server is an instance of its own; its tools are named
    `{component_id}__{remote_tool_name}`, so that servers cannot collide.
*   **Discovery.** The tools are discovered once, when the component is built. A server that is
    unreachable then does not fail the agent: its tool set is unavailable, with why in general
    terms, never the error, which may name hosts; a warning is logged.
*   **Description.** The parameter `description`, else what the server says of itself, else a
    generic text.
*   **Credentials.** `auth.secret` names a secret, resolved through the secret chain (see
    [SECURITY.md](SECURITY.md)); it is never a literal credential.
*   **Parameters.** `url` (required), `auth` (optional, `bearer`), `timeout_secs` (default 10),
    `requires_approval` (`true` by default, `false` or `"unless_read_only"`; see [What needs
    approval](#what-needs-approval)), `classification` and `description`.
*   **Calls.** Each call opens a connection, and so an MCP session, of its own. An interrupted call
    drops it, which ends the session; whether the remote tool stops is up to the server.

## Approvals

Some tool calls need a person's approval before they run. Nothing blocks: the agent suspends its
turn while it waits.

1.  **Interruption.** The agent registers the calls that require approval, runs whatever needs
    none, and moves to `WaitingForUser`. Its state, the waiting calls included, is stored.
2.  **Request.** The agent emits `ApprovalRequested`. The moderator routes it to the channel's
    `ApproverActor` only, which passes it on to the users - to everybody, so that all see what the
    agent waits for; only who may approve is asked. A request nobody taking part may approve is
    denied at once.
3.  **Answers.** A user approves or denies through their `UserActor`; the moderator routes the
    answer to the approver only. A rule of the user's may answer for them (see [Rules of a
    person](#rules-of-a-person)).
4.  **Decision.** The approver decides by its `ApprovalStrategy` (`Any`, `Majority`, `All`,
    `DenyOnFirst`), counting only those who may approve, and decides again when users join or leave.
    A request nobody decides times out after ten minutes. The approver's decision is the only way a
    decision reaches an agent, and an approval is over only once the approver says so, not when one
    user answered.
5.  **Resumption.** The agent runs the granted calls and continues its turn.

A denial or a timeout finishes the call at once, with `denied` or `timed_out`.

A turn that ends while it waits - cancelled, given up - withdraws its requests. The approver takes a
withdrawal only from the agent that asked, forgets the request without deciding it, and tells the
users, so that nobody is asked any longer. A stopped agent withdraws nothing; the approver answers
its pending requests as it leaves (see [CHANNELS.md](CHANNELS.md)).

The agent's turn is the record of what it waits for: the approver holds its requests only in memory
and starts empty after a restart. An agent that resumes a turn with calls still pending asks again,
with the same ids. The approver takes an unknown request up as new; one it still holds it only
announces again, keeping the answers given so far. Answers given before a restart are lost: the
users are asked again.

Messages that arrive while the agent waits are queued and processed in order once it resumes, so
its context stays consistent. Approval requests are not redacted by confidentiality: the approvers
see the arguments of the call they are asked to decide.

### What needs approval

What the model believes about who said what is no ground for a consequence: a participant can write
another's sender marker into their own message, and models believe it. A call runs under the agent's
permissions (`CallTool`), never under those of whom the model takes for the speaker, so a faked
marker gains the agent nothing it could not do anyway - but it can make the agent do it. What needs
approval follows from that:

*   **A tool that acts outside** needs it. `mcp_tool` requires approval for every tool of its
    server unless configured otherwise: `false` for none, which fits a server that only reads, or
    `"unless_read_only"` for all but the tools the server marks read-only. A tool without that mark
    counts as one that changes something. The mark is the server's word: trusted only where the
    configuration says so, and relaxing nothing beyond that. Read-only does not mean that nothing
    leaves the system: the arguments of a search reach the server all the same.
*   **Releasing** a note, a private note or something remembered to a wider circle needs it.
*   **What stays in the conversation** needs none: reading within what the agent may read and its
    channel may take in, the tools without an effect, and ending the turn - an agent put to sleep on
    someone's word is woken with `/wakeup`. A plan is one of them: it says what the agent intends,
    and every step with an effect is approved on its own. Whoever wants to see plans before they run
    sets a rule of a place that asks for `create_plan`.
*   **Notes and memory** need none either, although what the agent writes there may rest on a faked
    word and from then on stands in the agent's name. That is accepted: an approval for every note
    would leave notes unused, a note is shown to every member, and every entry records where it was
    made (see [MEMORY.md](MEMORY.md)).

Whoever approves decides on what the call does, not on the agent's account of why; what was said,
and by whom, the context panel shows.

### Rules of a place

Beside what a tool says of itself, the places an agent works in restrict its calls: the system
(`AppConfig::approvals`, see [CONFIGURATION.md](CONFIGURATION.md)), its session and its channel
(their `approval_rules`, set by whoever may change them). A rule (`ApprovalRule`) names the tool as
the model calls it, `*` for every tool, and optionally a regular expression per argument that its
whole value must match; it may expire. It decides one of two things:

*   **`deny`** - the call never runs. It is answered `forbidden` before anyone is asked, and again
    if the rule was set while the call waited for its approval.
*   **`ask`** - a person decides this very call, whatever the tool says: the request is *explicit*,
    and rules a person sets for themselves do not answer it.

A place cannot allow: there is nobody at a place who would stand for an approval. Of the rules of
all three places that apply to a call, the strongest decides - `deny` before `ask`. The
`ToolExecutor` reads them as they stand at every call; the approver is not involved. Patterns are
matched without backtracking and bounded in size; a rule whose pattern cannot be matched restricts
and never allows. A pattern is reliable for arguments with a structure of their own, not for one
that is a language, such as a shell command.

### Rules of a person

A person can give their answer in advance: "for my part, `ls` is always fine". Such a rule has the
same shape as a rule of a place, but it allows or denies - asking is what happens without one - and
reaches everywhere, one session or one channel. The rules are the person's, kept in their
`UserProfile`, which nobody else reads.

A rule is evaluated not by the approver, which has no right to approve anything of its own, but by
the person's `UserActor` when a request reaches it: if a rule of theirs applies, and they may
approve the agent's calls in the channel (asked first), the actor answers as their click would and
shows it to them. To the approver it is an answer of theirs like any other, marked with the rule for
the record. The `ApprovalStrategy` decides between people with opposite rules as between people who
clicked.

*   **Explicit requests** are answered by no rule.
*   **Answers come at once.** Under `Any`, a person's `allow` decides before anyone read the
    request: a rule weighs as much as the button.
*   **The brave mode** is a rule like any other: every tool, allowed in one session, for a while.
*   **A rule is derived from a call**: the approval card offers "and from now on", with the matcher
    the call's tool suggests. The person answers the call first; the rule is added after.
*   Rules are changed through the user's controller, which stores them first, announces the profile
    and drops those expired. A rule is bound to no agent: whether the person may approve bounds it.

### Who may approve

Who may approve a call is the `SecurityManager`'s to decide, and the approver enforces it:
`ApproveToolCall` carries the attributes of the agent - loaded, never taken from the request, which
the agent makes itself - and of the channel. The ownership policy grants it the agent's owner and
whom the agent's access list lets read it, who are responsible for it also in someone else's
channel; for an agent of the system, the channel's owner.

*   An answer counts only if its sender may approve, asked in the context the answer came with; any
    other is left out, logged and without telling the sender - only who may approve is offered the
    buttons, so such an answer comes from a client going round what it was shown.
*   The strategies count only those taking part who may approve, now. An answer of someone who left
    or may no longer approve does not count, either way. With nobody left who may approve, the
    request is denied.
*   The views offer it with `MemberView::may_approve`, which the web UI shows the buttons by.

### Checked before anyone is asked

Before a call is registered, the `ToolExecutor` decides how it is admitted (`ToolExecutor::admit`):
answered at once, put to a user, or run as it is. The actor only records what the executor decided.

*   **Policy.** A call the `SecurityManager` does not permit is refused before any approval is
    requested, so users are not asked about calls that would be refused anyway. It is the one place
    the policy is asked about the call.
*   **Validity.** A call that cannot run - arguments its tool rejects, a tool that does not exist -
    is answered at once instead of being put up for approval.

### Results

Every call ends with a result the model can act on.

*   A successful call's result is the tool's output.
*   Every other outcome is `{"status": ..., "message": ...}`, whose message says what happened and
    what to do next: `denied`, `forbidden` (without the reason, which stays in the log),
    `timed_out`, `declined`, `invalid_arguments`, `failed`, `not_executed` (the tool never ran),
    `interrupted` (it ran, but was broken off) and `unknown_tool` (with the tools that do exist).
    The status is stored with the result, so that refusals and failures can be told from results.
    The wording lives in `crate::tool`.
*   No call is left without a result. A turn that ends while calls of its step have none answers
    them as `not_executed`, saying why the turn ended.

A call that ran records when it started and ended, around the tool's own run, so that a client can
show the duration.

## Questions

A tool may ask a person before its call runs (`ToolProvider::questions`), and the answers are handed
to its `execute`. They are no messages, and the model learns of them only what the tool puts into
its result. The turn waits for them as for an approval. The built-in `ask_user` is the simplest such
tool: the model writes the questions and names whom to ask by their handles, and the result is the
answers. Its calls are presented as `questions`: frontends show the questions and, once answered,
the record of the answers, nothing raw.

1.  **Admission.** The executor asks the tool for the call's questions after `describe`, checks them
    (`Questions::check`) and puts them to the users of the channel: by handle, or the agent's owner.
    A handle that names no user of the channel, or an owner who does not take part, makes the call
    invalid. A call that needs approval is approved first, then asks. Whoever approves sees the
    arguments, never the answers.
2.  **Request.** The agent sends `QuestionRequested` under an id of the runtime's - not the model's
    call id, which some backends repeat. The moderator sends it to the users of the channel - to
    those asked alone if the questions are sensitive - never to the approver or other agents. Only
    those asked answer.
3.  **Answers.** The user's actor refuses what cannot count - questions not open or not put to them,
    responses not as the questions ask - before anything is sent; whether the user may answer in the
    channel at all (`AnswerQuestion`) the channel decides as it admits the answer. The answer goes
    to the asking agent alone, which takes it only if its sender was asked and the questions are
    still open, and checks the responses again; who answered it takes from the envelope, never from
    what was said. The first answer that counts settles the call.
4.  **Declining.** A user may decline, and a message of theirs written while questions put to them
    are open declines them all: the declines are sent before the message, so that the turn goes on
    and the message is heard after it. A declined call is answered `declined` without running.
5.  **Closing.** Once questions are no longer open, the agent tells those they were shown to, and
    every frontend closes its form. A cancel or a stop ends the waiting as for an approval.

The turn is the record of what it waits for: a turn resumed after a restart asks again, with the
same ids. While a call waits to run, its answers are not stored: a call ready to run when its turn
is resumed is answered as not executed, so nothing is lost.

Once the call has finished, its result keeps the record of who answered or declined, and what. The
record goes with the result into the transcript and the history and is published as the result is,
but is never given to a model.

### Sensitive questions

A tool may ask for what the model must not see - a one-time code, an account to pay to - and marks
such questions `sensitive`. Then:

*   they are shown to those asked alone: the request, its closing and the snapshot entry reach
    nobody else;
*   their answers are not stored and not logged: the result keeps no record of them, not even who
    declined, and a user's own actor does not echo them to their other frontends;
*   whom they are put to is the tool's to say, not the model's, so that the model cannot choose who
    is asked for a password;
*   the frontend masks the input and says that the answer goes to the tool alone - a mark the tool
    sets, which a model cannot set on the questions of `ask_user`;
*   the tool does not repeat the answers in its result - its own responsibility;
*   a transport that keeps envelopes must not keep them.

## User commands

A line that starts with `/` addresses the system rather than the agents: `/help`, `/wakeup [agent]`,
`/nudge <participant>`. A command is not a message: it never reaches an agent's context as something
said. Commands of the channel never enter the history; what an agent carried out on someone's word
does (see [Commands of an agent](#commands-of-an-agent)).

Commands are for people; agents act through tools. A command is always issued on behalf of a
particular person, by that person's actor (`UserActor::execute_command`), as messages and approvals
are; only `UserActor` issues commands. What a command does to the channel goes through the channel's
own, permission-checked operations; what it sends in the issuer's name, their actor sends, since an
actor sends only in its own name; a command of an agent is sent to the agent, which carries it out
itself.

### Recognised by the frontend

The client decides whether a line is a command and sends it as one rather than as a message; the
message API stays plain text. The rule is `caelumora_model::command::parse_input`, mirrored in the
web UI: a leading `/` starts a command, whose name runs to the first whitespace; the rest of the
line is handed to the command unparsed; `//` sends a literal `/`. Only this lexical rule is repeated
per frontend. What a command means, who may run it and what it answers is decided in one place.

### Commands are components

A `ChannelCommandProvider` contributes commands run on the channel, as a `ToolProvider` contributes
tools; the issuer is given as their actor. Commands come from two places:

*   `AppConfig::commands`, the global set - by default `/help`, `/wakeup` and `/nudge`; an empty
    list switches them off (see [CONFIGURATION.md](CONFIGURATION.md));
*   `SessionTemplate::commands`, for commands that belong to what a session is for, copied into the
    session, so a running session keeps its set.

Both are built once per session into a `CommandExecutor` shared by the session's channels - the
counterpart of an agent's `ToolExecutor`, which likewise receives ready providers and is no
component. Two commands of one name keep the session from starting. The executor is generic over the
facet: finding a command, refusing two of one name and asking the policy are the same whoever runs
the commands; only what a command is given to run with - the session, or an agent's turn - is the
facet's. It checks permissions itself: how a command runs is the command's business, and there is
nothing about running commands to try out.

### Seeing and running

Seeing and running a command are separate permissions: `ShowCommand` decides whether it appears in
`/help` and in completion, `ExecuteCommand` whether it runs. Both carry the command and the
channel's attributes. The ownership policy grants them whoever reads the channel, and a command for
the owners alone the channel's owner.

A command the user may not see is answered exactly like an unknown one, so hidden commands cannot be
found by trying names. Whatever the command does is checked on its own as well: `/wakeup` asks for
`WakeChannel`. `/nudge` asks nothing more: it asks someone to respond, as an `@`-mention does, which
asks nothing of the policy either.

### Private or channel-wide answers

A command answers the issuer alone (`/help`, `/nudge`) or the whole channel (`/wakeup`). A
channel-wide answer is announced by the issuer's actor as `CommandExecuted`, forwarded to the users
only, shown but never stored. The issuer gets the answer back from `execute_command` either way,
since the channel never routes an envelope back to its sender; the REST response carries it only
when it is private, since the web UI receives the announcement on its stream. A command of an agent
answers later, on the stream; an answer for the issuer alone is routed to them alone.

### Naming an agent

Every command names agents and participants alike (`ChannelLoop::agents_named`,
`participants_named`). The text is taken as a name first, compared as names are; only if it is
nobody's name is it taken as a handle, with or without its `@`, normalised as a mention is. A handle
is coarser than a name, so it can name several; a command then refuses with their names rather than
picking one.

## Commands of an agent

Some commands act on an agent rather than on the channel: `/skill` puts a skill into its transcript,
`/compact` compresses it. An agent's state in a channel has one writer, its actor there, so such a
command is not run by the session but carried out by the agent itself - an agent run here and an
external agent alike.

*   **Contributed by components.** A component of the personality offers them through
    `AgentCommandProvider`, as a tool is offered, carried out in the agent's `Turn`. Which commands
    an agent carries out follows from its personality alone. Two of one name, or one that a command
    of the session has, keep the agent from starting.
*   **Always addressed.** A command of an agent names its agent - `/compact @bob`; `@all` names
    every agent of the channel that carries it out. Without a handle, or with one that names no such
    agent, it is refused with the handles to choose from.
*   **Seen and asked for.** `ShowAgentCommand` and `ExecuteAgentCommand` carry the command and the
    attributes of the agent and of the channel. Under the ownership policy they are granted to
    whoever may write in the channel, unless the command is for the agent's owners alone: then to
    whoever answers for the agent, as approving its calls (see
    [SECURITY.md](SECURITY.md#the-ownership-policy)). Who may not see a command finds it unknown.
*   **Sent, then carried out.** The issuer's actor asks the session's commands first, then the
    agents of the channel. Offering asks the policy through the agent's executor directly, so it
    answers at once while the agent is busy; whether the issuer may ask for the command is the
    agent's to decide as it takes the request in. The channel admits a command request only from a
    user to an agent taking part, and the moderator routes it to that agent alone. A quiet channel
    delivers it as well.
*   **Where it came.** The agent takes in what it was told up to the command, carries it out, and
    takes in the rest, so that what was said before and after stands around it in its transcript as
    in the history. A command wakes nobody and is carried out asleep as well; the agent does not
    respond to it. One that comes during a turn waits for it.
*   **In a turn of its own** (see [AGENT-TURN.md](AGENT-TURN.md#a-command-turn)), in the request's
    contexts continued in the agent's name - its owner's rights, the issuer as initiator
    ([SECURITY.md](SECURITY.md#in-whose-name-an-agent-acts)). The executor asks there, in the
    context the request came with, whether the issuer may ask for it: the one place it is asked.
    Refused, the agent answers `CommandFailed` to the issuer. Carried out, the turn is taken over
    and stored; not carried out, it is dropped as if it had never begun.
*   **Begun.** Admitted, the agent tells the issuer alone that it began, whatever its answer's
    visibility: for `/compact`, the issuer sees it begin and everybody sees it done.
*   **Answered.** Carried out, the agent answers `CommandExecuted` - to the issuer alone if the
    answer is private, to every user otherwise; not carried out, `CommandFailed` to the issuer
    alone, with why. A command answers what it did, not what it read: nothing withholds its answer
    as a tool's result is withheld.
*   **Recorded.** What an agent carried out, with its arguments and on whose word, goes into the
    channel's history and the agent's - not what it answered, which may have been the issuer's
    alone. A command refused or failed did nothing and leaves no entry.
*   **Listed.** `/help` lists the agents' commands after the channel's, each with the handles of the
    agents that carry it out; the member snapshots carry them for completion.

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
| `ask_user` | `ask_user_tool` | `questions` (1-5, each with `header`, `question`, optional `choices` and `multi_select`), `ask` (handles) | no | `SHOWN` | Asks the users named and waits for their answers, which are its result (see [Questions](#questions)). |
| `create_plan` | `create_plan_tool` | `summary`, `steps` (each with `description`) | no | `SHOWN` | Submits a multi-step plan; a plan in progress is extended, not replaced. |
| `{id}__{name}` | `mcp_tool` | as the server declares | per `requires_approval`, yes by default | `SHOWN` | A tool of a remote MCP server. |
| `write_note` | `notes` | `text` | no | `SHOWN` | Notes something in the channel, visible to every participant. |
| `update_note` | `notes` | `id`, `text` | no | `SHOWN` | Changes the text of one of the agent's own notes; it keeps its number. |
| `revoke_note` | `notes` | `id` | no | `SHOWN` | Revokes one of the agent's own notes. |
| `read_notes` | `notes` | `channel` (optional) | no | `SHOWN` | Reads the notes of another channel, or of every readable channel. |
| `reclassify_note` | `notes` | `id`, `level`, `compartments` (optional) | yes | `SHOWN` | Reclassifies one of the agent's notes, never above the channel. |
| `write_private_note` | `private_notes` | `text` | no | `HIDDEN` | Notes something only the agent (and its owner) sees. |
| `update_private_note` | `private_notes` | `id`, `text` | no | `HIDDEN` | Changes the text of one of the agent's private notes. |
| `revoke_private_note` | `private_notes` | `id` | no | `HIDDEN` | Revokes one of the agent's private notes. |
| `reclassify_private_note` | `private_notes` | `id`, `level`, `compartments` (optional) | yes | `HIDDEN` | Reclassifies one of the agent's private notes, never above the channel. |
| `remember` | `agent_memory` | `text` | no | `HIDDEN` | Remembers something across channels and sessions. |
| `update_memory` | `agent_memory` | `id`, `text` | no | `HIDDEN` | Changes the text of something remembered that may be recalled here. |
| `forget` | `agent_memory` | `id` | no | `HIDDEN` | Forgets something remembered. |
| `reclassify_memory` | `agent_memory` | `id`, `level`, `compartments` (optional) | yes | `HIDDEN` | Reclassifies something remembered that may be recalled here, never above the channel. |
| `run_command` | `terminal_tool` | `command`, `wait_secs` (optional) | as configured, yes by default | `SHOWN` | Runs a shell command in the workspace and follows it; answers with its output and exit code, or with its id while it runs. |
| `command_output` | `terminal_tool` | `id`, `offset` (optional), `wait_secs` (optional) | no | `SHOWN` | What a command started in the channel printed since `offset`. |
| `kill_command` | `terminal_tool` | `id` | no | `SHOWN` | Ends a command started in the channel, and what it started. |
| `list_files` | `terminal_tool` | `path` (optional) | as configured; not under `"unless_read_only"` | `SHOWN` | Lists a directory of the workspace. |
| `read_file` | `terminal_tool` | `path`, `start_line`, `line_count` (optional) | as configured; not under `"unless_read_only"` | `SHOWN` | Reads a text file of the workspace, or the lines asked for. |
| `grep_files` | `terminal_tool` | `pattern`, `path` (optional), `max_matches` (optional) | as configured; not under `"unless_read_only"` | `SHOWN` | Searches the text files of the workspace for a regular expression. |
| `write_file` | `terminal_tool` | `path`, `content` | as configured, yes by default | `SHOWN` | Writes a text file of the workspace, its directories made. |
| `replace_in_file` | `terminal_tool` | `path`, `old_text`, `new_text`, `replace_all` (optional) | as configured, yes by default | `SHOWN` | Replaces text that occurs once - or every occurrence - in a text file. |
| `move_file` | `terminal_tool` | `from`, `to` | as configured, yes by default | `SHOWN` | Moves or renames a file or directory, never over something. |
| `delete_file` | `terminal_tool` | `path` | as configured, yes by default | `SHOWN` | Deletes a file or an empty directory of the workspace. |
| `publish_file` | `terminal_tool` | `path`, `name`, `description` (optional) | never | `SHOWN` | Offers a file of the workspace for download at its message, as a link to it as it is now. |
| `publish_artifact` | `artifact_tool` | `path`, `name`, `description` (optional) | as configured, yes by default | `SHOWN` | Publishes a file of the workspace as a version of an artifact (see [Artifacts](#artifacts)). |
| `list_skills` | `skills` | - | no | `SHOWN` | Lists the skills the agent may load here ([SKILLS.md](SKILLS.md)). |
| `load_skill` | `skills` | `name` | no | `WITHOUT_RESULT` | Gives the skill's text, classified as the skill; recorded as knowledge. |
| `create_skill` | `skills` | `name`, `description`, `body`, `scope` (`session` or `user`) | as configured, yes by default | `SHOWN` | Writes a skill of the agent's owner, classified as the turn's label. |
| `update_skill` | `skills` | `name`, `description` (optional), `body` (optional) | as configured, yes by default | `SHOWN` | Changes a skill of the owner's, raised first to what the turn has seen. |
| `read_history` | `channel_history` | `channel`, `last` (optional), `query` (optional) | no | `SHOWN` | Reads the latest messages of another channel, or those containing a text. |

`stop`, `bye` and `silence` end the turn with the current tool step. Notes, private notes and agent
memory are described in [MEMORY.md](MEMORY.md). How frontends show the calls of each tool is said
by its `ToolPresentation`.

### The workspace

`terminal_tool` gives an agent its session's workspace, where its channel may use it
([CHANNELS.md](CHANNELS.md#its-workspace)): its files, and a shell where its storage gives one. The
workspace is reached through the turn's views on every call, before anything is sent; a call where
there is none, or a command where its storage has no shell, fails and says so. What its access list
grants decides the rest: the tools that only read files need reading (`GetWorkspace`), the others
and every shell tool changing (`WriteWorkspace`) - a command may change anything its place reaches.
Every tool is offered, and one that changes fails where the list grants reading alone; the prompt
part says so beforehand ([AGENT-TURN.md](AGENT-TURN.md#its-workspace)). Which calls need approval is
configured as for `mcp_tool`: all - the default -, none, or all but those that only read files.

**Files.** The file tools work over the workspace's `Terminal`, whatever its storage, without a
shell. A path is relative to the workspace's top; one that leaves the workspace is the model's to
correct, as is a file that is not there or not text. Reading is bounded, and says where to read on.
A replacement must find its text exactly once, unless every occurrence is to be replaced; it reads,
replaces and writes, so what changed the file in between is overwritten. Nothing is moved over,
which would be a deletion, and the workspace's top is neither moved nor moved over. What was read
is recorded as the turn's knowledge, a file's content by its digest (see
[MEMORY.md](MEMORY.md#recording-what-was-read)). The approver reads a change whole, and a rule
derived from an approved change covers the file, whatever is written into it - for a move, both
places.

`publish_file` offers a file for download at the message that calls it: a link to the file as it is
now, with its digest, downloaded through the channel's workspace by whoever may use it there. It
changes nothing and hands nothing to anyone who could not download the file already, so it reads,
and never needs approval, whatever is configured. What is to be kept as it was offered is published
with `publish_artifact`.

**Commands.** A command runs in the workspace's directory, every one anew - a `cd` does not carry
over - and is seen only from the channel it was started in. A call follows its command for
`wait_secs`; one still running then is handed back with its id, and the model follows it with
`command_output` or ends it with `kill_command`. The result is classified as the workspace. The
approver reads the command whole; following and ending a command need no approval, since only a
command that was approved is followed or ended.

A call dropped while it waits for its command - the turn cancelled or stopped - kills the command,
in a task of its own, bounded and logged, that the drop does not await. It is the deliberate
exception to "a tool binds whatever it starts to its own future": a command lives with the storage,
so that one handed back as running outlives the call, and one left running by a dropped call would
go on writing into the workspace after the turn that started it.

A command handed back as running is ended by the model, or by a user who may change the workspace's
files in the channel; the agent sees `killed` the next time it follows the command. How long a
command lives and when the runtime ends it is in [WORKSPACE.md](WORKSPACE.md#commands).

### Artifacts

`publish_artifact` (component `artifact_tool`) publishes a file of the session's workspace as a
version of an artifact: a fixed copy, kept in an artifact storage beyond the workspace, downloaded
at the message that published it. `publish_file` offers a link to a file as it is now; a model is
told the difference in their descriptions - a link for a look now, an artifact for a result to be
kept.

The file is read from the workspace as the file tools read it and handed to the `ArtifactManager`.
It publishes in the name of whoever the agent acts for: its owner, who makes the artifact
(`CreateArtifact`) or adds a version where the artifact's list lets them (`WriteArtifact`), asked
before anything is kept. The version is read by the members of the channel it is published in. An
artifact is published into the session's scope under its name, made with its first version and given
one more by every later publication under that name. It is classified as the workspace, fixed with
its first version; a later version from a workspace classified otherwise is refused. A storage takes
nothing above its `destination`. Publishing changes what is kept, so it is approved where changes
are; the approver reads which file is published as which artifact.

The members of the channel download the version published there, wherever they act; whom the
artifact's own list grants it read every version; its owner reads every version, shares it and keeps
it beyond the session. One that is not kept goes with the session, its content too (see
[STORAGE.md](STORAGE.md) and [SECURITY.md](SECURITY.md#ownership)).

### Commands

| Command | Of | Component | Arguments | Answer | What it does |
|---|---|---|---|---|---|
| `/help` | the channel | `help_command` | `[command]` | private | Lists the commands the user may use - the channel's and the agents' -, or describes one. |
| `/wakeup` | the channel | `wakeup_command` | `[agent]` | channel-wide | Wakes the sleeping agents of the channel, or the one named. |
| `/nudge` | the channel | `nudge_command` | `<participant>` | private | Asks the participant named to respond to the conversation as it stands, without a message ([CHANNELS.md](CHANNELS.md#nudging-someone)). |
| `/skill` | the agent | `skills` | `@agent <name>` | private | Gives the agent a skill: its text goes into the agent's transcript in the issuer's name ([SKILLS.md](SKILLS.md)). |
| `/skills` | the agent | `skills` | `@agent` | private | Lists the skills the agent may load here. |
| `/compact` | the agent | `compaction` | `@agent` | channel-wide | For the agent's owners: compacts its context as its `CompactionStrategy` does ([AGENT-TURN.md](AGENT-TURN.md#compaction)). |
