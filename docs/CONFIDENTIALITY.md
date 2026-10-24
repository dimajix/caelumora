# Confidentiality

How sensitive information in Caelumora is and for whom it is meant: the classification of channels,
the flow rule that decides what one channel may read of another, and which parts of what an agent
produces only its owner sees. Who may do what is a separate question, answered in
[SECURITY.md](SECURITY.md); the notes and memory these rules apply to are described in
[MEMORY.md](MEMORY.md). What is not done yet is listed under [Open questions](#open-questions).

## Classification

What may flow from one channel to another is decided by classifications
(`caelumora_model::confidentiality`), not by who takes part in the channels. A channel is a
construct of a certain trustworthiness, and the decision must not change when someone joins or
leaves.

A `Classification` is a **level** and a set of **compartments**. The levels (`Confidentiality`) are
ordered, from the least to the most sensitive:

| Level | Meaning |
|---|---|
| `public` | anyone may read it |
| `internal` | for the organisation |
| `confidential` | |
| `strictly_confidential` | |

A **compartment** is an area information belongs to - a topic, a group, a customer. The compartments
of a classification are its **audience**: who may read it. Adding one widens the audience. A
classification without compartments is **general**: readable by everyone of a sufficient level.
`public` takes no compartment; a classification that pairs them is refused, also when it is read
from a file or a request.

```yaml
classification:
  level: confidential
  compartments: [hr]
```

### Compartments are configured

Which compartments exist, and which may read which, is configured by the administrator in the
section `compartments` (see [CONFIGURATION.md](CONFIGURATION.md)):

```yaml
compartments:
  - name: management
    reads: [hr]
  - name: hr
    reads: [finance]
  - name: finance
```

Every compartment reads itself. The read relation is **closed transitively**: what HR wrote may
contain what HR read from finance, so management, which reads HR, reads finance too. A compartment
that reads one that is not defined, or is defined twice, is a configuration error.

### Channels

Every channel has a classification, set by its owner when creating it, in the session template or
the request; the default is `internal` without compartments. A classification that names a
compartment not configured is invalid (`require_classifiable`), checked before anything is stored,
so that a typo does not go unnoticed. Which session a channel is in decides whether one channel
reaches another at all, not how anything is classified (see [Reading another
channel](#reading-another-channel)).

### Messages

Every message carries a classification. It is not optional, and a `Message` has no default
(`Message::new(classification)`), so that no code path can leave a message classified by accident.

*   **Its sender classifies it.** An agent's messages - answers, tool results, what its model is
    given - carry its turn's label (see [What a turn writes as](#what-a-turn-writes-as)); until a
    message is labelled, it has the strictest general classification. A user's message carries the
    channel's, unless the user classifies it otherwise.
*   **The channel admits it.** Putting a message into a channel is
    `ConfidentialityPolicy::may_publish`, asked by the `ChannelLoop` for every message of every
    event as it takes the event in - the one gate for every actor, which asks nothing itself for
    what it sends. An event with a message it may not admit is written nowhere, and its sender is
    told why (`MessageRejected`, see [CHANNELS.md](CHANNELS.md)). The rules allow it if every
    compartment is configured and the classification flows into the channel; that only the actor
    acting puts something into a channel is the channel's own check of the event, not a question of
    the flow.
*   **Released when written.** By the flow rule a message is never classified above its channel. One
    classified otherwise is released: at a lower level, or to further compartments besides those the
    channel's are covered by - an `{hr, finance}` message in an `{hr}` channel is readable from
    finance from the start. A user may release what they write; the channel admits it as any
    message. Changing a classification afterwards is [reclassifying](#reclassification) it.
*   **Modified, admitted again.** A message its author modifies carries a classification of its
    own, the one it had unless the author chooses another, and is admitted as a new message is,
    after whether its author may modify it.
*   **Taken back, not erased.** A message its author takes back keeps its place without what it
    said, and the agents are no longer given it. What it said is still recorded: in the chains of
    every participant that heard it, in the contexts of the answers produced from it - readable by
    whoever may read the agent's context - in what was noted or remembered of it, and with a remote
    agent that was given it. Taking a message back changes what the conversation says from now on,
    not what was disclosed; a secret written by mistake is not removed by it.

### Reclassification

The classification of a channel or a message can be changed afterwards (`ActionType::Reclassify`).
Lowering it - a lower level, a compartment added to the audience, or none left - *releases* what it
holds to a wider circle; raising it retracts nothing already read. The circle is one of
classification: a release adds no readers to the channel it is in - an agent reads a released entry
of another channel only where it may read that channel (see [Reading another
channel](#reading-another-channel)). Who may reclassify is the `AccessPolicy`'s: the ownership
policy allows a channel's owner to reclassify the channel and every message in it, and a message's
author their own message. To what is the flow's: a message never above its channel, and only with
configured compartments.

*   **A channel** (`ChannelLoop::reclassify`) is stored and announced. Lowering it releases what is
    in it: every message is fitted into the new classification (`Classification::fitted_into`: its
    level at most the channel's, the channel's compartments added to its audience), since the agents
    in the channel carry what they know into what they write next. Messages released further before
    keep their wider audience. This is recorded as one entry in the channel's history, which every
    message said so far is read through; the agents' state keeps the messages as they were, whose
    labels never exceed the channel's anyway. Raising a channel above someone in it is refused
    rather than anyone removed (see [Clearances](#clearances)).

    A turn that is running keeps the classification it began with, and what it saw is released by
    nobody: the owner decided on what the channel held, not on what the turn read elsewhere. So
    lowering a channel ends its turns:
    *   once stored, the channel cancels its turns and stays quiet. Lowered means that what the
        channel held, classified as it was, may not be put into it as it is now (`may_publish`, in
        the name of whoever reclassifies); raising it ends nothing;
    *   every agent asks before each step of a turn whether the turn still flows into the channel as
        it is now. One that does not is given up as `Lowered` rather than taken over - also when it
        was cancelled, began just before the change, or is resumed after a restart: no tool runs any
        more, and nothing it says from then on reaches the channel. Its model is given of it only
        which tools it called and how they ended, classified as the channel is now: what it read
        after the lowering, or in a step it did not complete, was never released (see
        [AGENT-TURN.md](AGENT-TURN.md#beginning-and-ending-a-turn)). The users are told why;
    *   an external agent's work is given up the same way: an answer under way publishes nothing of
        what came so far; an open task begun before is cancelled at the remote agent and forgotten,
        whenever it would be followed, said or taken up again. What the remote agent was given is
        released with the channel, as an agent's transcript is; what is given up is the work begun
        under the classification the channel had - one rule for both kinds of agent.

    The web UI warns when a change releases the channel.
*   **A message** is a change to what was said, made by a participant, and goes as a modification
    goes (see [CHANNELS.md](CHANNELS.md#changing-what-was-said)): the actor of whoever reclassifies
    sends it without content, and the channel asks as it admits it - `Reclassify` (its author, or
    the channel's owner for someone else's), then whether the new classification may flow into the
    channel. The owner reclassifies as a participant: one who is not has no actor in the channel and
    is refused. A message taken back is not reclassified - it says nothing that could be released.

    It is recorded as an entry of its own in the channel's history and in the history and transcript
    of every agent that holds the message: the message stays what it was said as, and is read as it
    stands now. So a turn begun afterwards is labelled by the message as it stands: a message
    released no longer raises the label of what the agents write next. A turn under way keeps what
    it has seen.
*   **A note or a memory entry** is reclassified by its agent, through a tool that asks for approval
    (`reclassify_note`, `reclassify_private_note`, `reclassify_memory`), as a change of its chain
    taken over with the turn. The ownership policy allows it to the author - the agent calling -,
    the flow never above the turn's channel; a memory entry only where it may be recalled, and it is
    recalled where its new classification may be read. Agents do not reclassify channels or
    messages: people do.

Who reclassified what is not recorded: that is an audit log
([140](../issues/140-audit-log.md)). The chains keep the change and its place in what happened, not
who made or approved it.

## The flow rule

One relation decides everything (`Classification::flows_to`): information classified S may flow into
a destination classified D if

*   D's level is at least S's, and
*   S has no compartments, or D has at least one and every compartment of D is covered by one of
    S's - the same, or one it may read.

For a destination with one compartment this reads "any of the source's compartments will do"; for a
channel with two, what flows into it must be meant for both, since both audiences see it. What is
general flows into any destination of a sufficient level; what has an audience never flows into a
general one.

## Clearances

Who may take part in a channel, or read it, is decided by what they are cleared for. A `Clearance`
is a level and at most one compartment: it sees what is classified up to that level, in that
compartment - or, without one, what is general. Alice may be cleared for `internal` of finance and
`confidential` of HR. A person *belongs* to compartments, while a channel is *published* to them; so
a clearance is not judged by the flow rule, but by its own (`Clearances::admits`):

*   what is public is seen by everybody;
*   what is general, by a clearance without compartment of at least its level;
*   what has compartments, by a clearance of at least its level whose compartment reads one of
    them - the same, or one it may read.

A clearance follows from roles. The `roles` section of the configuration names the roles and the
clearances each grants; the user provider gives users their roles. A user's clearances are all their
roles'; an agent's are its owner's, since it must not see more than the person responsible for it.
Both come with the `SecurityContext`: its roles - those of its owner (`owner_id`), not of whoever
the caller acts for -, and the roles its caller is limited to, which have to clear as well. A user
whose roles grant nothing sees only what is public. Without a `roles` section, clearances are not
checked. The system is cleared for everything, as it may do everything
(`ConfidentialityQuestions::cleared_for`, issue 312): what the runtime does in its own name -
cleaning up, deleting with a scope - reaches what it acts on.

### Personalities limit their agents

A personality may name `roles` its agents are limited to: an agent is cleared only if its owner is
*and* those roles are. They narrow the owner's clearance and never widen it - whoever may write a
personality cannot clear anyone through it. A personality that names none limits nothing, except an
**external** one: what is given to an external agent leaves the system, so naming none limits it to
no roles, which see only what is public (`Personality::limiting_roles`). The same limit holds back
an agent run here whose model or tools are outside the system.

An external agent, once let into a channel, is given everything said in it, as a cleared person
hears everything; that it may receive it was decided at the door. What it answers is classified as
a turn's messages are: as what its work has seen - everything it was given in the channel, and what
the remote agent brings in itself, classified as its component declares, else as the channel -
never above the channel as it was when the work began. A declaration does not lower what it was
given: a remote agent declared public, told what is internal, answers internal. Whether what it
brings in itself may enter the channel is asked when it joins and again with every request; a
request it may not answer in the channel as it is now is given up, and the remote agent is not
called.

The question is `ConfidentialityPolicy::cleared`, asked in the context of whoever admits the
participant, with the roles the channel resolves the participant to - a user's own, an agent's
owner's, as the participant's controller holds them now. For an agent whose personality limits it,
it is asked once more with those roles, and both must allow. It is asked

*   when a participant is added to a channel - also each agent of a session made from a template,
    once the session is,
*   for every participant of a channel created on request or from a template, before anything is
    stored,
*   for every participant when a channel is reclassified - raised above someone in it, it is refused
    rather than anyone removed,
*   and for a user reading a channel, by the roles of their context and an agent's limit besides: a
    channel they are not cleared for is not found, and not listed - even for its owner.

## Workspaces

A workspace has one classification for all its files, which carry none of their own. Who works on it
is cleared as a reader of a channel so classified would be:

*   A person, by the workspace - its owner, or whom its access list lets, wherever they act: cleared
    for it when they make it, raise it and use it. Where they act decides nothing: what they read
    reaches only them.
*   An agent, in a channel classified L of a session it is attached to, where the workspace,
    classified W, and the channel flow both ways - W into L, since what is read there reaches the
    channel; L into W, since what was seen in the channel may be written there. Asked as an actor
    putting something into a place is (`may_publish` both ways, in the agent's name), at every use,
    besides the access list. With compartments and a read relation that is not symmetric this is
    stricter than equality. A participant is cleared for L, so nothing else is asked.

A storage - of workspaces or of content - bounds what may be put into it by its `destination`:
making or raising a workspace there, before the clearance, and keeping an artifact's version there
ask one question, whether its classification may be published into the destination
(`require_storable`); a storage without one takes anything. A classification says who may read a
workspace; it does not keep a command in it - how far a storage keeps its workspaces apart is in
[WORKSPACE.md](WORKSPACE.md#storages).

Reclassifying a workspace is its owner's: lowering releases every file in it; which channels may use
it follows at their next use. Nothing in it is fitted - there is nothing inside to fit.

## Reading another channel

Reading from another channel is decided per entry: per message of its history, per note, per memory
entry. The policy is asked once what of the other channel may be copied into the reading one,
`ConfidentialityPolicy::copyable`: the source channel and its session as privacy sees it
(`SessionPrivacy`), and the reading `Place` - its channel, its classification, its session. It
answers data, not a decision per entry: how far the reader reads the channel, the whole, what was
released to it, flowing into its classification, or nothing (`Reading`, issue 421). Each entry is
then judged by its classification against that answer (`Reading::admits`, behind
`may_copy_information`) - in memory, or by the store as a condition over the level and compartments
of the messages of a history (`Filter::readable`), so that a page counts only what may be read. It
is asked in the name of the agent that reads - its owner's - because the agent passes on what it
reads. A channel may always be copied into itself. Otherwise the rules allow it if

*   the entry's classification flows into the reader's, and
*   both channels belong to the same session, or the source's session lets the reader's read it
    (`SessionPrivacy::permits`).

That is a question of classification and privacy alone. Whether the agent may reach the other
channel at all is asked before, as for a user: `GetChannel`, which an agent may if its owner may
read the channel or it takes part in it itself. What it knows itself of a channel - what it
remembered there, its private notes - it reaches through itself, without `GetChannel`, and copies by
the two conditions alone: it heard or wrote it, whoever takes part in that channel now.

So a channel reads the entries of another that were released to it, even where it may not read that
channel as a whole - but only where its agent may reach that channel: a release adds no readers. A
channel is listed among the readable ones only if it may be read as a whole, by its own
classification; one that may not, and of which nothing was released, reads as absent, so that
channels cannot be probed.

The second condition is privacy, not confidentiality: with classifications alone, two `internal`
channels of different sessions could read each other, but whether the people of a session agreed to
be read elsewhere is not a question of how sensitive something is. Every session has one privacy
policy, taken over from its template:

*   `private` - the default: what was said in it is read only within it;
*   `same_owner` - other sessions of the same owner may read it;
*   `shared_audience` - other sessions may read it whose people - their users and owner - all take
    part in this one.

It holds whatever the classification: even what is `public` does not leave a private session. A
policy has to be transitive - what it lets one session read, that session passes on by its own
policy without breaking the first one's promise; `same_owner` and `shared_audience` are, a grant to
a named session would not be.

Only the session's owner changes its policy, and everyone who takes part sees it (see
[CHANNELS.md](CHANNELS.md#sessions)). What an agent remembered keeps the policy it was said under
([MEMORY.md](MEMORY.md)): a policy that lets more sessions read holds for what is said from then on,
one that lets fewer takes back what it let go. How an agent reads other channels is described in
[MEMORY.md](MEMORY.md).

## What only the owner sees

Beside the classification there is a second axis: whether anyone but the agent's owner sees
something at all. That is ownership, not sensitivity, and a flag of its own (`Visibility`):
`channel` - published to the channel - or `owner` - never published; the agent sees it, and its
owner in the context panel. The channel records it, and gives it to nobody else (issue 417).

### Tool calls

A `Tool` declares its `ToolVisibility`: three flags for its calls - `call`, whether that the tool
was called is published, `arguments`, whether what it was called with is, and `result`, whether what
it returned is. Neither the arguments nor the result are seen by more than the call.

| Constant | `call` | `arguments` | `result` | Meaning |
|---|---|---|---|---|
| `SHOWN` | channel | channel | channel | the default |
| `WITHOUT_RESULT` | channel | channel | owner | others see that the tool was called and with what, but not what it returned |
| `CALL_ONLY` | channel | owner | owner | others see that the tool was called, but not with what |
| `HIDDEN` | owner | owner | owner | the whole call stays with the agent |

Every call is stamped with the flags its tool declares at that moment, the result takes them over,
and both are stored with what applied then. Which built-in tools use which flags is listed in
[COMPONENTS.md](COMPONENTS.md#the-components).

### Tool results

A tool result carries a classification of its own. In this order: a tool classifies a result by
what it read or the source it drew on; a tool can be configured with one for all its results (e.g.
the `classification` of an `mcp_tool`); otherwise a result is the channel's.

**Before the model, not after it.** A result that may not flow into the channel - `may_publish`, in
the agent's name, since the result enters its context in the channel - is withheld from the agent:
the model is told only that it was withheld and how it is classified, and the value is kept nowhere.
So the answer text, which carries no label, cannot quote it. A result that may flow keeps its
classification: below the channel's, it is released.

The channel admits a message only if its tool results may enter it too. Another channel reads a
message by the message's classification; a released result in a message it may not read stays
unread.

### What leaves through tools

What an agent writes into the arguments of a call leaves the system where the tool has a remote
side. A tool set says where they go - the classification its remote side is cleared for
(`ToolSet::destination`) - and the `ToolExecutor` judges it when it assembles the tools of a turn,
with the channel the turn begins in: a set whose destination the channel may not flow to (asked with
the agent's owner's rights) is offered as unavailable, without its tools, so that the model and the
web UI say why, and a call of a tool the turn was not offered is answered as unknown, also by name.
As for an external agent, the remote side is judged at the door, not per call: nothing in the
channel is classified above it.

An `mcp_tool` is cleared for what its `destination` says, `public` unless configured: the server's
word is trusted only where an operator gave it. A set without a destination keeps its arguments in
the system, or is bounded otherwise - `terminal_tool`'s storage by its own ceiling, the workspace by
the flow rule at every call. A turn keeps the tools it began with; a channel raised during it
re-judges them at the next turn.

### Reasoning

`Message::reasoning_visibility` says who sees a message's reasoning. It is `owner` if

*   the personality's `reasoning_visibility` is `owner` (default `channel`),
*   the turn has seen the agent's private knowledge - its private notes or its agent memory, or
*   a call the reasoning leads to has `owner` arguments: of the message's own calls once it is
    complete, and, while it streams, of every tool the agent could still call - because reasoning
    comes before the call and may hold what goes into it. A result that is the owner's does not
    count: the reasoning cannot hold what comes back after it.

Otherwise the reasoning is published and carries the channel's classification, as everything the
agent derives does. Knowledge read from other channels adds nothing: a channel reads only what may
flow into it. The answer text itself carries no flag.

What a model is given of reasoning is a separate question from who may read it (see
[AGENT-TURN.md](AGENT-TURN.md#reasoning-given-back)).

### Redaction

The agent keeps the full calls in its context, and sends what it says as it is: the channel records
it whole (issue 417). What anyone else is given of it is redacted by one rule
(`Message::published`):

*   a call whose `call` is the owner's is left out;
*   a call whose `arguments` are the owner's has them set to `null`;
*   a result that is the owner's is emptied, the record of the answers to its questions included;
*   reasoning that is the owner's is dropped;
*   of a message with nothing left, nothing is given: it is recorded as gone to nobody.

The channel applies it as the event enters: the moderator decides on the published form, and that
is what is delivered and announced. Every read of the history applies it too, for a reader who is
not the sender (`Recorded::as_seen_by`, see [STORAGE.md](STORAGE.md#histories)): REST, the
snapshot, `read_history` and an agent catching up are given the published form. The context panel
(`ReadAgentContext`) shows the agent's own, full view.

Approval requests are not redacted: the approvers see the arguments of a call they are asked to
decide.

### What the model is told

Since the answer text is always published, the model is told what it must not quote: a successful
result that is the owner's reaches it with a note that only it sees the result
(`tool::successful_result`), and the private notes' prompt part and tool descriptions say not to
reveal them.

## What a turn writes as

Everything an agent's turn writes - its messages, the tool message with its results, its notes and
private notes, what it remembers - is classified by the turn's *label* (`Work::label`), stamped when
it is written, so that the reading rule can follow it into other channels; a later change of the
channel changes none of it.

The label is the join of the classifications of everything the turn has seen so far (`Work::see`):
it starts at `public` and rises with

*   every message of the transcript - those it began with, and what others said while it ran - and
    every tool result in them,
*   every prompt part, as its `PromptProvider` classifies it: a part that holds only knowledge, such
    as the notes or what is recalled from memory, by that knowledge; the system prompt of
    `static_system_prompt_provider` by its parameter `classification`, `public` if left out - who
    writes something confidential into a personality's prompt classifies it; a part that does not
    say counts as the channel,
*   every piece of knowledge read: notes, private notes and memory by the join of the entries read,
    another channel's history by the join of the messages given,
*   every tool result, once the model is given it. A built-in tool whose answer is derived only from
    what the model asked - `calculate`, `echo`, `write_note`, `remember`, ... - classifies it as the
    turn.

The join (`Classification::join`) is the higher level and the audience in common. It is never above
the channel the turn began in: everything the turn sees flows into the channel, and where the join
is not known to flow there too - no compartment in common, or only by which compartments read which,
which only the rules know - the label is the channel's classification. So the label is the channel's
classification at worst, and lower where the turn has seen less: what an agent remembers from a turn
that saw only internal things in a confidential channel stays readable in internal channels.

What the label was made of can be told afterwards: every prompt part and every knowledge read of a
message's context records the classification it had when it was read. Which of them set the label is
derived when it is asked for, against the message's classification as it is now
(`LabelCauses`); nothing about it is stored:

*   a part that was not classified counted as the channel, and set the label to the channel's;
*   if the classified parts had no audience in common, the label is the channel's, set by the parts
    with an audience;
*   otherwise the parts at the label's level set it. The label's audience is what the parts'
    audiences have in common, which each part shows.

Notes and private notes carry their classification, and other channels read them note by note; the
agent is shown the private notes of every other channel of the session that its current channel may
read. Agent memory entries carry the label they were written with, and are recalled only in channels
that may read it. The reading channel is judged by the classification the turn began with: what the
turn may read is bounded by its channel, not by its label.

This is sound because an agent never sees anything that may not flow into its channel, and what it
writes is classified at least as everything it has seen: everything it wrote may be read wherever
all of that may, and no wider.

"Private" means private from other agents and other users, not from the agent's owner: the owner
sees private notes in the context panel, which is theirs alone.

## Who applies the rules

The rules of this document - the flow rule, a session's consent to being read from beyond it, which
compartments may be given, the clearances of the configured roles - are decided by a
`ConfidentialityPolicy`, apart from who may do what, which is the `AccessPolicy`'s.
`ConfidentialityRules`, built from the `compartments` and `roles` sections of the configuration, is
the one Caelumora applies. It decides from the context, the classifications and the places alone:

*   `may_publish` - what is so classified may be put into a destination so classified;
*   `copyable` - what of a channel may be copied to a place, by the source session's privacy, as
    data: nothing, the whole, or what flows into the place's classification by the flow rule;
*   `cleared` - someone, by their roles, is cleared for what is so classified;
*   and it names the configured compartments, by which a classification is valid at all
    (`require_classifiable`) - validation, not a permission.

An act that moves information asks its own permission of the `AccessPolicy` and the flow; in
which order is [SECURITY.md](SECURITY.md#operations-that-check-themselves)'s - the flow of the
actor's own request may come first, since it tells them nothing of others. The two are orthogonal: a
superuser who may act against both would need a hook in both, which is accepted. A policy outside
the binary, such as Open Policy Agent, would state both.
`allow_all` allows every action, but no flow the rules refuse.

## Open questions

*   **Sources that classify.** No built-in tool classifies its results, and an MCP server cannot yet
    say how its results are classified, only be configured with one classification for all of them
    ([139](../issues/139-mcp-result-classification.md)). Configuring a tool with a classification is
    trusted: it is not checked as one that can be given.
*   **Roles from directories.** Roles come from the configuration of the user provider; mapping
    directory groups to roles is [142](../issues/142-roles.md).
*   **Sessions.** An agent's memory is so far the one way across sessions; whoever is added to a
    channel later sees what it drew from other sessions
    ([143](../issues/143-joining-a-channel-with-foreign-knowledge.md)).
*   **What an external agent keeps.** A session's privacy promises that what was said stays in it.
    An external agent with a memory of its own carries what it heard into whatever it does next: the
    rules decide whether it is let in, not what it keeps
    ([179](../issues/179-external-agents-via-a2a.md)).
*   **What leaves through tools.** A provider that sends out but declares no destination is not held
    back; a future tool with a remote side has to declare one.
*   **The answer text.** It carries no label and is always published; the model is only told what
    not to quote ([66](../issues/66-channel-level-in-prompt.md)). Keeping private knowledge out of
    it relies on the model.
*   **Knowledge beyond the session.** A retrieval of notes
    ([59](../issues/59-retrieval-of-notes.md)) will have to carry and respect the classification of
    where it came from; a search index must not become a way around the flow rule.
*   **One enforcement for component state.** Every component that reads across channels asks
    `may_copy_information` itself; when notes become generic component state
    ([65](../issues/65-generic-component-state.md)), the core is to apply the rules.
