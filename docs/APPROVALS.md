# Approvals and Questions

A tool call may wait for a person: for their approval before it runs, or for their answers to
questions its tool asks. Either way the agent's turn waits, nothing blocks, and what it waits for is
recorded with the turn. This document describes both. How tool calls are run is in
[TOOLS.md](TOOLS.md).

## Approvals

Some tool calls need a person's approval before they run; the agent suspends its turn while it
waits.

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
    user answered. The approver speaks in its own name, answered for by the owner of its channel,
    with whoever began the call as initiator.
5.  **Resumption.** The agent runs the granted calls and continues its turn.

A denial or a timeout finishes the call at once, with `denied` or `timed_out`.

### What is recorded of an approval

Who decided is recorded at the call, for the readers of its message (issue 419). Every answer says
whose it is (`ApprovalResponse::decisions`): a person's answer their own part alone - who, named
as then, the rule of theirs that answered if one did, and when (`ApprovalDecision`) -, set by their
actor; the channel lets in no answer of a person that says anything else. The approver keeps each
part with the answer, and its decision says whose answers made it: every counted answer that agrees
with it - the first who approved under `Any`, all of them under `All`, the majority under
`Majority`, who denied under `DenyOnFirst` -, in the order they were given; none where it timed out
or nobody may approve. The agent keeps that with the call from the decision on, whatever the call
waits for next, and takes it into the call's result as it ends (`ToolCallResult::approval`): so the
tool message in the transcript and the channel's history says by whom the call was approved or
denied - whether it was, its status says. It is published with the call even where its result is
withheld, since every user was told the decision, and never given to the model. The chat shows it
at the call ("Approved by …", "Denied by …"). The act itself, for an audit across channels, is
[140](../issues/140-audit-log.md)'s. Approvals are no items of the history (issue 417): the
history records the requests, decisions and withdrawals only to deliver them again to an actor
whose inbox fell behind (issue 223), and no page shows them.

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
*   **Writing into the workspace** an attachment heard needs it: `copy_attachment` is always asked,
    as a write there is. Showing an attachment again, reading one or attaching it to the answer
    stays in the conversation (issue 282).
*   **What stays in the conversation** needs none: reading within what the agent may read and its
    channel may take in, the tools without an effect, and ending the turn - an agent put to sleep on
    someone's word is woken with `/wakeup`. A plan is one of them: it says what the agent intends,
    and every step with an effect is approved on its own. Whoever wants to see plans before they run
    sets a rule of a place that asks for `create_plan`.
*   **Notes and memory** need none either, although what the agent writes there may rest on a faked
    word and from then on stands in the agent's name. That is accepted: an approval for every note
    would leave notes unused, a note is shown to every participant, and every entry records where it
    was made (see [MEMORY.md](MEMORY.md)).

Whoever approves decides on what the call does, not on the agent's account of why; what was said,
and by whom, the context panel shows.

### Rules of a place

Beside what a tool says of itself, the places an agent works in restrict its calls: the system
(`AppConfig::approvals`, see [CONFIGURATION.md](CONFIGURATION.md)), its session and its channel
(their `approval_rules`, set by whoever may change them). A rule (`ApprovalRule`) names the tool as
the model calls it, `*` for every tool, and optionally per argument how its value is compared
(`ArgumentMatcher`): a regular expression its whole value must match, for a bash command line the
words its commands begin with ([Rules on commands](#rules-on-commands)), or the whole value as it
is, bounded in length; it may expire. It decides one of two things:

*   **`deny`** - the call never runs. It is answered `forbidden` before anyone is asked, and again
    if the rule was set while the call waited for its approval.
*   **`ask`** - a person decides this very call, whatever the tool says: the request is *explicit*,
    and rules a person sets for themselves do not answer it.

A place cannot allow: there is nobody at a place who would stand for an approval. Of the rules of
all three places that apply to a call, the one that decides the strongest of it decides - `deny`
before `ask` (`ApprovalRule::decide`). The `ToolExecutor` reads them as they stand at every call;
the approver is not involved. Patterns are matched without backtracking and bounded in size; a rule
whose pattern cannot be matched restricts and never allows. A pattern is reliable for arguments
with a structure of their own, not for one that is a language, such as a shell command.

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
    the call's tool suggests - by default derived from its parameters (`ToolMatcher::proposed`):
    every argument exactly, a command line of one command by its first two words, as far as they
    do not expand. Exactly is an escaped pattern, which the person can widen, or for a value too
    long for one the value as it is. A rule that could not be kept is not offered. The person
    answers the call first; the rule is added after.
*   Rules are changed through the user's controller, which stores them first, announces the profile
    and drops those expired. A rule is bound to no agent: whether the person may approve bounds it.

### Rules on commands

An argument that is a bash command line - `command` of `run_command` - is compared by what it runs
rather than by a pattern: the rule names the words its commands begin with
(`{ "shell": ["git", "status"] }`), for a named tool. The model reads the line into its simple
commands (`approval/shell.rs`): split at `;`, `&&`, `||`, `|` and line breaks, each a list of
words with quoting and escapes removed. A word that expands - a glob, a `~` - equals no word named.

*   **A chain is judged by its parts.** A rule that allows applies if every command begins with its
    words, one that asks or denies if any does: a rule for `git` allows `git status && git diff`,
    one that denies `rm` denies `ls && rm x`. Of several arguments a rule names, each must match.
*   **What cannot be read is not judged.** A line is unjudgeable where bash would run other words
    than those written, or do more than run them: substitution and expansion (`$`, a backtick),
    redirection, a background job, subshells and groups, a leading assignment, a command beginning
    with a reserved word (`while`, `do`, `if`, ...) or with a word that expands, an unclosed quote,
    a line too long. No rule allows such a line; a rule that asks asks, and one that denies asks
    too, rather than refuse every line with a `$` in it. A person's rule that thus only asks
    answers nothing: the person decides.
*   **Not a boundary.** Words are compared as written. A program reached by another path
    (`/bin/rm`) or through another (`env rm`, `sh -c "rm"`, `find -exec rm`) is not seen: a rule
    that denies a program is a hint. A rule that allows is safe against all of them - the words
    must begin with what it names - but wider than it reads: programs take options after their
    operands, so one for `rm x` allows `rm x -rf /`. The boundary is where the command runs, the
    workspace's storage ([WORKSPACE.md](WORKSPACE.md)).
*   **A pattern stays a pattern.** Nothing but the rule says that an argument is a command line; a
    pattern on one is compared as text, and `ls.*` matches `ls; rm x`. What a tool declares of its
    parameters serves the rule it proposes, not the judging.

### Who may approve

Who may approve a call is the `AccessPolicy`'s to decide, and the channel enforces it as it
admits an answer (issue 312): `ApproveToolCall` carries the attributes of the agent the answer
names, loaded, never taken from the answer or the request. The ownership policy grants it the
agent's owner and whom the agent's access list lets read it, who are responsible for it also in
someone else's channel; for an agent of nobody, which everybody may read, nobody (issue
[491](../issues/491-every-entity-has-an-owner.md)). No agent is the system's (issue
[477](../issues/477-only-the-system-acts-as-the-system.md)).

*   An answer enters the channel only if its sender may approve, asked in the context the answer
    came with; any other is refused and its sender told (`MessageRejected`), as everything the
    channel does not let in. The approver counts an answer only for the agent that asked; one
    naming another agent is left out and logged. A decision - granted, denied, timed out - enters
    the channel only from an approver.
*   The strategies count only those taking part who may approve, now. An answer of someone who left
    or may no longer approve does not count, either way. With nobody left who may approve, the
    request is denied.
*   The views offer it with `ParticipantView::may_approve`, which the web UI shows the buttons by.

### Checked before anyone is asked

Before a call is registered, the `ToolExecutor` decides how it is admitted (`ToolExecutor::admit`):
answered at once, put to a user, or run as it is. The actor only records what the executor decided.

*   **Policy.** A call the `AccessPolicy` does not permit is refused before any approval is
    requested, so users are not asked about calls that would be refused anyway. It is the one place
    the policy is asked about the call.
*   **Validity.** A call that cannot run - arguments its tool rejects, a tool that does not exist -
    is answered at once instead of being put up for approval.

## Questions

A tool may ask a person before its call runs (`ToolProvider::questions`), and the answers are handed
to its `execute`. They are no messages, and the model learns of them only what the tool puts into
its result. The turn waits for them as for an approval. The built-in `ask_user` is the simplest such
tool: the model writes the questions and names whom to ask by their handles, and the result is the
answers. Its calls are presented as `questions`: frontends show the questions and, once answered,
the record of the answers, nothing raw.

1.  **Admission.** The executor asks the tool for the call's questions after `describe`, checks them
    (`Questions::check`) and puts them to the users of the channel: by handle, the agent's owner
    (`Addressee::Owner`), or whoever caused the call (`Addressee::Initiator`, the initiator of the
    turn - the customer a bank's agent serves, not the bank). A tool that asks for something
    sensitive names the owner or the initiator itself, never a handle the model chose. A handle that
    names no user of the channel, or an owner or initiator who does not take part as a user, makes
    the call invalid. A call that needs approval is approved first, then asks. Whoever approves sees
    the arguments, never the answers.
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
5.  **Leaving.** One asked who leaves the channel is no longer asked, and has neither answered nor
    declined. While others asked are left, the questions stay open for them; once nobody asked is
    left, they go unanswered: the call is answered as not executed, without running and without a
    record of answers - also a call still waiting for its approval, whose request is withdrawn.
6.  **Closing.** Once questions are no longer open, the agent tells those they were shown to, and
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
