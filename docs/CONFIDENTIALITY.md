# Confidentiality

This document describes how sensitive information in Caelumora is and for whom it is meant: the
classification of channels, the flow rule that decides what one channel may read of another, and
which parts of what an agent produces only its owner sees. Who may do what is a separate question,
answered in [SECURITY.md](SECURITY.md); the notes and memory these rules apply to are described in
[MEMORY.md](MEMORY.md). The model is introduced in steps
([133](../issues/133-classification-model.md)); what is not done yet is listed under [Open
questions](#open-questions) at the end.

## Classification

What may flow from one channel to another is decided by classifications
(`caelumora_model::confidentiality`, issue 133), not by who takes part in the channels. A channel is
a construct of a certain trustworthiness, and the decision must not change when someone joins or
leaves.

A `Classification` is a **level** and a set of **compartments**.

The levels (`Confidentiality`) say how sensitive information is. They are ordered, from the least to
the most sensitive:

| Level | Meaning |
|---|---|
| `public` | anyone may read it |
| `internal` | for the organisation |
| `confidential` | |
| `strictly_confidential` | |

A **compartment** (`Compartment`) is an area information belongs to - a topic, a group, a customer.
The compartments of a classification are its **audience**: who may read it. Adding one widens the
audience. A classification without compartments is **general**: readable by everyone of a sufficient
level. `public` takes no compartment; a classification that pairs them is refused, also when it is
read from a file or a request.

Written down, a classification is an object with a `level` and, if there are any, its
`compartments`:

```yaml
classification:
  level: confidential
  compartments: [hr]
```

### Compartments are configured

Which compartments exist, and which may read which, is configured by the administrator in the
top-level section `compartments` of the configuration (`Compartments`, see
[CONFIGURATION.md](CONFIGURATION.md)):

```yaml
compartments:
  - name: management
    reads: [hr]
  - name: hr
    reads: [finance]
  - name: finance
```

Every compartment reads itself. The read relation is **closed transitively** when the configuration
is read: what HR wrote may contain what HR read from finance, so management, which reads HR, reads
finance too. A compartment that reads one that is not defined, or is defined twice, is a
configuration error.

### Channels

Every channel has a classification (`Channel::classification`), set by its owner when creating it,
in the session template or the request; the default is `internal` without compartments
(`Classification::CHANNEL_DEFAULT`). Giving a channel a classification is asked as
`ActionType::Classify`, before anything is stored; the ownership policy refuses a compartment that
is not configured, so that a typo does not go unnoticed. Which session a channel is in decides
whether one channel reaches another at all, not how anything is classified (see [Reading another
channel](#reading-another-channel)).

### Messages

Every message carries a classification (`Message::classification`, issue 133). It is not optional,
and a `Message` has no default: a message is built with `Message::new(classification)`, so that no
code path can leave a message classified by accident.

*   **Its sender classifies it.** An agent's messages - its answers, tool results, what its model is
    given - carry the classification its turn began with (`label` in `actor_impl/agent.rs` for
    everything its model produces; until then an `LLMClient` builds on `llm::unlabelled_message`,
    the strictest general classification). A user's message carries the channel's, unless the user
    classifies it otherwise.
*   **The channel admits it.** Putting a message into a channel is `ActionType::Publish {
    classification, actor, channel }`, asked by the `ChannelLoop` for every message of every event
    that enters it (`ChannelLoop::admits`) - the gate for every actor. Wherever it is only a
    question, it is asked as `AccessQuestions::may_publish` (issue 158); `UserActor::send_message`
    requires it as part of sending. An event with a message it may not admit is dropped and logged.
    The ownership policy allows it if the actor is the one acting (the context's caller), every
    compartment is configured, and the classification flows into the channel.
*   **Released when written.** By the flow rule a message is never classified above its channel. One
    classified otherwise is released: at a lower level, or to further compartments besides those the
    channel's are covered by - an `{hr, finance}` message in an `{hr}` channel is readable from
    finance from the start. A user may release what they write (`UserActor::send_message` asks
    `Publish` before it sends, so that a refusal reaches the user as
    `ChannelError::ClassificationRefused`). Changing a classification afterwards is
    [reclassifying](#reclassification) it.
*   **Modified, admitted again.** A message its author modifies
    ([234](../issues/234-modify-revoke-and-retry-messages.md)) carries a classification of its own,
    the one it had unless the author chooses another, and enters the channel as a message does:
    `ActorEvent::MessageModified` carries the message, so `ChannelLoop::admits` asks `Publish` for
    it, and `UserActor::modify_message` asks before it sends.
*   **Taken back, not erased.** A message its author takes back keeps its place without what it
    said, and the agents are no longer given it. What it said is still recorded: in the chains of
    every participant that heard it, in the contexts of the answers produced from it - readable by
    whoever may read the agent's context - in what was noted or remembered of it, and with a remote
    agent that was given it. Taking a message back changes what the conversation says from now on,
    not what was disclosed; a secret written by mistake is not removed by it.

### Reclassification

The classification of a channel or a message can be changed afterwards (`ActionType::Reclassify {
target, current, new, channel, attributes }`, issue 133). Lowering it - a lower level, a compartment
added to the audience, or none left - *releases* what it holds to a wider circle; raising it
retracts nothing already read. The ownership policy allows a channel's owner to reclassify the
channel and every message in it, and a message's author to reclassify their own message - a message
never above its channel, and only with configured compartments.

*   **A channel** (`ChannelLoop::reclassify`, REST `PUT …/channels/{cid}/classification`) is stored
    (`UpdateChannel`) and announced as `GlobalEvent::ChannelUpdated` with
    `ChannelUpdateEvent::ClassificationChanged`. Lowering it releases what is in it: every message
    is fitted into the new classification (`Classification::fitted_into`: its level at most the
    channel's, the channel's compartments added to its audience, none if the channel is general),
    since the agents in the channel carry what they know into what they write next. Messages
    released further before keep their wider audience.

    A turn that is running keeps the classification it began with, and what it saw is released by
    nobody: the owner decided on what the channel held, not on what the turn read elsewhere. So
    lowering a channel ends its turns (issue 141):
    *   once stored, the channel cancels its turns and stays quiet, as `ChannelLoop::cancel` does.
        Lowered means that what the channel held, classified as it was, may not be put into it as it
        is now (`ActionType::Publish`, in the name of whoever reclassifies); raising it ends
        nothing;
    *   every agent asks before each step of a turn whether the turn still flows into the channel as
        it is now. One that does not is given up (`GivenUpReason::Lowered`, with the new
        classification) rather than taken over - also when it was cancelled, also when it began just
        before the change or is resumed after a restart: no tool runs any more, and nothing of it
        reaches the channel or the conversation. Its messages stay as a branch no longer active; the
        agent is told which tools ran, not with what (see
        [AGENT-TURN.md](AGENT-TURN.md#beginning-and-ending-a-turn)); the users get a `TurnFailed`
        saying why.
    *   an external agent's work is given up the same way (issue 212): an answer under way, which
        the lowering cancels, publishes nothing of what came so far; an open task begun before is
        cancelled at the remote agent and forgotten, when it would be followed, when what came of it
        would be said, or when a request would take it up again. What the remote agent was given is
        released with the channel, as an agent's transcript is; what is given up is the work begun
        under the classification the channel had - one rule for both kinds of agent, and what the
        web UI warns of.

    The web UI says so when a change releases the channel.
*   **A message** (`UserActor::reclassify_message`, REST `PUT …/messages/{mid}/classification`) is
    a change to what was said, made by a participant, and goes as a modification goes (see
    [CHANNELS.md](CHANNELS.md#changing-what-was-said), issue
    [239](../issues/239-reclassified-message-not-in-agents-state.md)): the actor of whoever
    reclassifies - its author, or the channel's owner for someone else's - asks `Reclassify` and
    sends `ActorEvent::MessageReclassified`, without content; the channel asks again as it admits
    it. The owner reclassifies as a participant: one who is not has no actor in the channel and is
    refused. A message taken back is not reclassified - it says nothing that could be released.

    It is recorded as an entry of its own, `Reclassified` (issue 197), in the channel's history and
    in the history and transcript of every agent that holds the message: the message stays what it
    was said as, and is read as it stands now (`MessageHistory::messages`, `message`,
    `transcript::as_they_stand`). So a turn begun afterwards is labelled by the message as it
    stands: a message released no longer raises the label of what the agents write next. A turn
    under way keeps what it has seen, as it does when a channel is raised.

    Lowering a channel records one `HistoryEntry::Fitted`, which every message said so far is read
    through - in the channel's history alone: the agents' state keeps the messages as they were,
    whose labels never exceed the channel's anyway.

*   **A note or a memory entry** is reclassified by its agent, through a tool that asks for approval
    (`reclassify_note`, `reclassify_private_note`, `reclassify_memory`): a change of its chain
    (`NoteChange::Reclassify`, `MemoryChange::Reclassify`), taken over with the turn. The policy
    allows it to the author - the agent calling - never above the turn's channel; a memory entry
    only where it may be recalled. A reclassified memory entry is recalled where its new
    classification may be read. Agents do not reclassify channels or messages: people do.

Who reclassified what is not recorded: that is an audit log ([140](../issues/140-audit-log.md));
the chains of the history, of notes and of memory keep the change and its place in what happened,
but not who made it or who approved it.

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

Who may take part in a channel, or read it, is decided by what they are cleared for (issue 133). A
`Clearance` is a level and at most one compartment: it sees what is classified up to that level, in
that compartment - or, without one, what is general. Alice may be cleared for `internal` of finance
and `confidential` of HR. A person *belongs* to compartments, while a channel is *published* to
them; so a clearance is not judged by the flow rule, but by its own (`Clearances::admits`):

*   what is public is seen by everybody;
*   what is general, by a clearance without compartment of at least its level;
*   what has compartments, by a clearance of at least its level whose compartment reads one of
    them - the same, or one it may read.

A clearance follows from roles ([142](../issues/142-roles.md)). The `roles` section of the
configuration names the roles and the clearances each grants (`Roles`); the user provider gives
users their roles - for now by its own configuration. A user's clearances are all their roles'; an
agent's are its owner's, since it must not see more than the person responsible for it. A user whose
roles grant nothing sees only what is public. Without a `roles` section, clearances are not checked.

### Personalities limit their agents

A personality may name `roles` its agents are limited to (issue 179): an agent is cleared only if
its owner is *and* those roles are. They narrow the owner's clearance and never widen it - whoever
may write a personality cannot clear anyone through it. A personality that names none limits
nothing, except an **external** one: what is given to an external agent leaves the system, so naming
none limits it to no roles, which see only what is public (`Personality::limiting_roles`). The same
limit holds back an agent run here whose model or tools are outside the system.

An external agent, once let into a channel, is given everything said in it, as a cleared person
hears everything; that it may receive it was decided at the door. What it answers is classified as
a turn's messages are (issue 212): as what its work has seen - everything it was given in the
channel, and what the remote agent brings in itself, classified as its component declares, else as
the channel - never above the channel as it was when the work began (`Work::label`,
`Classification::join_within`). A declaration does not lower what it was given: a remote agent
declared public, told what is internal, answers internal. Whether what it brings in itself may
enter the channel is asked when it joins and again with every request; a request it may not answer
in the channel as it is now - lowered since - is answered with `TurnFailed`, and the remote agent
is not called.

The question is `ActionType::JoinChannel { participant, roles, channel }`: the channel knows the
roles of a participant from what it resolves the participant to (`Member::clearances` in
`caelumora_runtime::channel`) - a user's own, an agent's owner's - and the policy maps them to
clearances. For an agent whose personality limits it, it is asked once more with those roles, and
both must allow (`Member::require_cleared`). A user's roles are read from the store each time: they
change at a login, past the controllers. It is asked

*   when a participant is added to a channel (`ChannelError::NotCleared`),
*   for every participant of a channel created on request or from a template
    (`SessionError::NotCleared`), before anything is stored,
*   for every participant when a channel is reclassified - raised above someone in it, it is refused
    rather than anyone removed,
*   and for a user reading a channel (`AccessQuestions::may_see_channel`): a channel they are not
    cleared for is not found, and not listed - even for its owner.

## Workspaces

A workspace (issue [253](../issues/253-workspace.md)) has one classification for all its files,
which carry none of their own: one container, as issue
[169](../issues/169-changing-classified-content.md) proposes. Who works on it is cleared as a
reader of a channel so classified would be:

*   Its owner, outside a session - unless it lies in a session's scope: cleared for it
    (`JoinChannel` with the workspace in the channel's place) when they make it, raise it and use
    it.
*   A member of a channel classified L, in a session it is attached to, where the workspace,
    classified W, and the channel flow both ways - W into L, since what is read there reaches the
    channel; L into W, since what was seen in the channel may be written there. Asked as an actor
    putting something into a place is (`Publish` of W into L and of L into W, in the member's name -
    an agent's, as it reads every other place, its owner's), at every use. With compartments and a
    read relation that is not symmetric this is stricter than equality. A member is cleared for L,
    so nothing else is asked.

A storage bounds what may be put into it (issue [251](../issues/251-open-terminal.md), part B2):
its `destination`, the classification a workspace in it may have at most - where its files go, and
for a storage with a shell where what a command reads can go, since a command reaches whatever its
host or container reaches. Making or raising a workspace there asks `Publish` of its classification
into the destination, before the clearance (`WorkspaceError::StorageNotCleared`). A `local` storage
with a shell is bounded to public unless it names its own; one without a shell, and without a
`destination`, is not bounded.

The isolation between workspaces is the storage's, not the classification's: a `local` shell runs
on Caelumora's host as its user, and reaches every workspace of every storage there, and Caelumora's
own files. An `open_terminal_shared` storage keeps all its workspaces in one instance, so a command
in one reaches every other, and whatever the instance's network reaches; the instance hands its
commands its own key, which Caelumora blanks in what it starts, but which a command can still read
from the server's process - and with it reach every command and file of the instance, of every
channel. One classification per such storage, and a storage per classification where there are
several. A classification says who may read a workspace; it does not keep a command in it.

Reclassifying a workspace is its owner's (`UpdateWorkspace`, `Classify`): lowering releases every
file in it; which channels may use it follows at their next use. Nothing in it is fitted - there is
nothing inside to fit.

## Reading another channel

Reading from another channel is decided per entry: per message of its history, per note, per memory
entry. It is `ActionType::ReadChannel { reader, source }`, each side a `Place`: the channel's id,
how the entry - or the reading channel - is classified, and the channel's session as privacy sees it
(`SessionPrivacy` - its id, policy, owner and people, issue 134), asked with
`AccessQuestions::may_channel_read`. It is asked in the name of the agent that reads - the security
context it acts in, its owner's - because the agent passes on what it reads (issue 131). A channel
may always read itself. Otherwise the ownership policy allows it if

*   the entry's classification flows into the reader's, and
*   both channels belong to the same session, or the source's session lets the reader's read it
    (`SessionPrivacy::permits`).

So a channel reads the entries of another that were released to it, even where it may not read that
channel as a whole. Whether it may, is the question asked with the source channel's own
classification: a channel is listed among the readable ones (`SessionView::channels`) only then, and
one that may not be read as a whole and of which nothing was released reads as absent, so that
channels cannot be probed.

The second condition is privacy, not confidentiality ([134](../issues/134-session-privacy.md)): with
classifications alone, two `internal` channels of different sessions could read each other, but
whether the people of a session agreed to be read elsewhere is not a question of how sensitive
something is. Every session has one privacy policy, stored with it as `PrivacyConfig` and taken over
from its template:

*   `private` - the default: what was said in it is read only within it;
*   `same_owner` - other sessions of the same owner may read it;
*   `shared_audience` - other sessions may read it whose people - their users and owner - all take
    part in this one.

It holds whatever the classification: even what is `public` does not leave a private session. A
policy has to be transitive - what it lets one session read, that session passes on by its own
policy without breaking the first one's promise; `same_owner` and `shared_audience` are, a grant to
a named session would not be (`caelumora_model::privacy`).

Only the session's owner changes its policy (`UpdateSession`), and everyone who takes part sees it
(see [CHANNELS.md](CHANNELS.md#sessions)). What an agent remembered keeps the policy it was said
under ([MEMORY.md](MEMORY.md)): a policy that lets more sessions read holds for what is said from
then on, one that lets fewer takes back what it let go.

How an agent reads other channels - the turn's views, `read_notes`, `read_history` - is described in
[MEMORY.md](MEMORY.md).

## What only the owner sees

Beside the classification there is a second axis: whether anyone but the agent's owner sees
something at all. That is ownership, not sensitivity, and it is a flag of its own (`Visibility`):
`channel` - published to the channel - or `owner` - never published; the agent sees it, and its
owner in the context panel.

### Tool calls

A `Tool` declares its `ToolVisibility` (issues 54, 133): two flags for its calls - `call`, whether
the call itself - that the tool was called - is published, and `content`, whether its arguments and
its result are.

| Constant | `call` | `content` | Meaning |
|---|---|---|---|
| `SHOWN` | channel | channel | the default |
| `CALL_ONLY` | channel | owner | others see that the tool was called, but not with what |
| `HIDDEN` | owner | owner | the whole call stays with the agent |

The agent stamps every call with the flags its tool declares at that moment, and the result takes
them over from the call. Both are stored with what applied then. Which built-in tools use which
flags is listed in [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

### Tool results

A tool result carries a classification of its own (`ToolCallResult::classification`, issue 133): a
tool classifies each of its results by what it read or the source it drew on (`ToolOutput`), a tool
can be configured with one for all of its results (`Tool::classification`, e.g. the `classification`
of an `mcp_tool`), and otherwise a result is the channel's. The `GuardedToolExecutor` applies them
in this order.

**Before the model, not after it.** A result that may not flow into the channel - asked as
`ActionType::Publish`, in the agent's name, since the result enters its context in the channel - is
withheld from the agent: the model is given the status `withheld` and a message that names the
classification instead, and the value is kept nowhere. So the answer text, which carries no label,
cannot quote it. A result that may flow keeps its classification: below the channel's, it is
released.

The channel admits a message only if its tool results may enter it too (`ChannelLoop::admits`).
Another channel reads a message by the message's classification; a released result in a message it
may not read stays unread.

### Reasoning

`Message::reasoning_visibility` says who sees a message's reasoning. It is `owner` if

*   the personality's `reasoning_visibility` is `owner` (default `channel`),
*   the turn has seen the agent's private knowledge - its private notes or its agent memory
    (`Turn::saw_private_knowledge`), or
*   a call the reasoning leads to has `owner` content: of the message's own calls once it is
    complete, and, while it streams, of every tool the agent could still call - because reasoning
    comes before the call.

Otherwise the reasoning is published and carries the channel's classification, as everything the
agent derives does. Knowledge read from other channels adds nothing: a channel reads only what may
flow into it.

The answer text itself carries no flag.

What a model is given of reasoning is a separate question from who may read it: no model is given
another actor's reasoning, and of the agent's own only what its model's rule keeps (issue 63, see
[AGENT-TURN.md](AGENT-TURN.md#reasoning-given-back)).

### Redaction

The agent keeps the full calls in its context. What it publishes is redacted in one place,
`Turn::send_event` (`Message::published`):

*   a call whose `call` is the owner's is left out;
*   a call whose `content` is the owner's has its arguments set to `null` and its result emptied,
    the record of the answers to its questions included (issue 165);
*   reasoning that is the owner's is dropped;
*   a message with nothing left is not sent.

The channel history is built from these events. History, REST, the event stream, `read_history` and
the moderator therefore only ever see the published form. The context panel (`ReadAgentContext`)
shows the agent's own, full view.

Approval requests are not redacted: the approvers see the arguments of a call they are asked to
decide.

### What the model is told

Since the answer text is always published, the model is told what it must not quote (issue 66):

*   A successful result whose content is the owner's reaches the model wrapped as `{"result": ...,
    "note": "Only you see this result ..."}` (`tool::successful_result`).
*   The private notes' prompt part and tool descriptions say not to reveal them.

## What a turn writes as

Everything an agent's turn writes - its messages, the tool message with its results, its notes and
private notes, what it remembers - is classified by the turn's *label* (`Turn::label`, issue 168),
stamped when it is written, so that the reading rule can follow it into other channels; a later
change of the channel changes none of it.

The label is the join of the classifications of everything the turn has seen so far
(`Work::see`): it starts at `public` and rises with

*   every message of the transcript - those it began with, and what others said while it ran - and
    every tool result in them,
*   every prompt part, as its `PromptProvider` classifies it (`Prompt::classification`): a part that
    holds only knowledge, such as the notes or what is recalled from memory, by that knowledge; the
    system prompt of `static_system_prompt_provider` by its parameter `classification`, `public` if
    left out - who writes something confidential into a personality's prompt classifies it; a part
    that does not say counts as the channel,
*   every piece of knowledge read (`KnowledgeRef::classification`): notes, private notes and memory
    by the join of the entries read, another channel's history by the join of the messages given,
*   every tool result, once the model is given it. A built-in tool whose answer is derived only from
    what the model asked - `calculate`, `echo`, `note`, `remember`, ... - classifies it as the turn;
    a result the tool does not classify is, as before, classified as the tool is configured, else as
    the channel.

The join (`Classification::join`) is the higher level and the audience in common. It is never above
the channel the turn began in: everything the turn sees flows into the channel, and where the join
is not known to flow there too - no compartment in common, or only by which compartments read which,
which only the rules know (`Classification::surely_flows_to`) - the label is the channel's
classification. So the label is the channel's classification at worst, which is what everything was
classified as before, and lower where the turn has seen less: what an agent remembers from a turn
that saw only internal things in a confidential channel stays readable in internal channels.

What the label was made of can be told afterwards: every prompt part (`PromptRef`) and every
knowledge read (`KnowledgeRef`) of a message's context records the classification it had when it was
read. The context of a message tells it, and which parts set the label (issue 170) - derived when it
is asked for, against the message's classification, nothing about it is stored:

*   a part that was not classified counted as the channel, and set the label to the channel's;
*   if the classified parts had no audience in common, the label is the channel's, set by the parts
    with an audience;
*   otherwise the parts at the label's level set it. The label's audience is what the parts'
    audiences have in common, which each part shows.

A message reclassified since is compared with what it is classified now.

In detail:

*   **Notes** (issue 17) and **private notes** (issue 51) carry their classification
    (`NoteChange::Add`, `Note::classification`). Other channels read them note by note; the agent is
    shown the private notes of every other channel of the session that its current channel may read
    (`AccessQuestions::may_channel_read`).
*   **Agent memory** (issue 62) entries carry the label they were written with, and are recalled
    only in channels that may read it (`AgentMemory::recalled`, which asks
    `AccessQuestions::may_channel_read` like the private notes, issue 131). The reading channel is
    judged by the classification the turn began with: what the turn may read is bounded by its
    channel, not by its label.

This is sound because an agent never sees anything that may not flow into its channel, and what it
writes is classified at least as everything it has seen: everything it wrote may be read wherever
all of that may, and no wider.

"Private" means private from other agents and other users, not from the agent's owner: the owner
sees private notes in the context panel, which is theirs alone (issue 54).

## Who applies the rules

The rules of this document - the flow rule, a session's consent to being read from beyond it, which
compartments may be given, the clearances of the configured roles - are `ConfidentialityRules`
(`caelumora_model::confidentiality`), built from the `compartments` and `roles` sections of the
configuration. They are a building block, not a policy: the `SecurityManager` decides every
confidentiality action, and the ownership policy decides them by these rules, together with what is
its own
- who acts, who may reclassify what - and with the system overriding them as it overrides
  everything.

Confidentiality and ownership are not always cleanly separable - a superuser who may act against the
rules is a decision about both - so a policy is not handed a fixed confidentiality part it can only
add to; it applies these rules, other rules, or none
([138](../issues/138-confidentiality-policy-in-ownership.md)). A policy outside the binary, such as
Open Policy Agent ([24](../issues/24-opa-security-manager.md)), states its rules itself, with the
compartments and roles as its data. `allow_all` applies none: it allows every flow.

## Open questions

The classification model is introduced in steps ([133](../issues/133-classification-model.md)); it
is complete. What is still open:

*   **Sources that classify.** No built-in tool classifies its results, and an MCP server cannot yet
    say how its results are classified, only be configured with one classification for all of them
    ([139](../issues/139-mcp-result-classification.md)). Configuring a tool with a classification is
    trusted: it is not asked as `Classify`.
*   **Roles from directories.** Roles come from the configuration of the user provider; mapping
    directory groups to roles, and roles of real user providers, is [142](../issues/142-roles.md).
    Events about a channel without content - a reclassification, say - still reach whoever may read
    the channel, cleared or not.
*   **Sessions.** An agent's memory is so far the one way across sessions; whoever is added to a
    channel later sees what it drew from other sessions
    ([143](../issues/143-joining-a-channel-with-foreign-knowledge.md)).
*   **What an external agent keeps.** A session's privacy promises that what was said stays in it.
    An external agent with a memory of its own runs on, and carries what it heard into whatever it
    does next: the rules decide whether it is let in, not what it keeps
    ([179](../issues/179-external-agents-via-a2a.md)).
*   **What leaves through tools.** What an agent writes into the arguments of a remote tool leaves
    the system with no rule asked; only what comes back is classified
    ([180](../issues/180-outbound-flow-unchecked.md)). A personality's roles hold such an agent back
    as a whole.
*   **The answer text.** It carries no label and is always published; the model is only told what
    not to quote ([66](../issues/66-channel-level-in-prompt.md)). Keeping private knowledge out of
    it relies on the model.
*   **Whose rights.** Reading is asked in the name of the agent's owner. How an agent's rights
    combine with those of the user it acts for is open
    ([12](../issues/12-user-and-agent-permissions.md)).
*   **Knowledge beyond the session.** Knowledge kept across sessions or shared between agents
    ([18](../issues/18-knowledge-sharing.md)) and retrieval of notes
    ([59](../issues/59-retrieval-of-notes.md)) will have to carry and respect the classification of
    where it came from; a search index must not become a way around the flow rule.
*   **One enforcement for component state.** When the state of notes becomes generic component state
    ([65](../issues/65-generic-component-state.md)), the rules are to be applied by the core, not
    re-implemented per component. Today every component that reads across channels asks
    `AccessQuestions::may_channel_read` itself (issue 131).
