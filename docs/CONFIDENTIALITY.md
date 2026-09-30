# Confidentiality

This document describes how sensitive information in Caelumora is and for
whom it is meant: the classification of channels, the flow rule that decides
what one channel may read of another, and which parts of what an agent
produces only its owner sees. Who may do what is a separate question,
answered in [SECURITY.md](SECURITY.md); the notes and memory these rules
apply to are described in [MEMORY.md](MEMORY.md). The model is introduced in
steps ([133](../issues/133-classification-model.md)); what is not done yet is
listed under [Open questions](#open-questions) at the end.

## Classification

What may flow from one channel to another is decided by classifications
(`caelumora_model::confidentiality`, issue 133), not by who takes part in the
channels. A channel is a construct of a certain trustworthiness, and the
decision must not change when someone joins or leaves.

A `Classification` is a **level** and a set of **compartments**.

The levels (`Confidentiality`) say how sensitive information is. They are
ordered, from the least to the most sensitive:

| Level | Meaning |
|---|---|
| `public` | anyone may read it |
| `internal` | for the organisation |
| `confidential` | |
| `strictly_confidential` | |

A **compartment** (`Compartment`) is an area information belongs to - a
topic, a group, a customer. The compartments of a classification are its
**audience**: who may read it. Adding one widens the audience. A
classification without compartments is **general**: readable by everyone of
a sufficient level. `public` takes no compartment; a classification that
pairs them is refused, also when it is read from a file or a request.

Written down, a classification is an object with a `level` and, if there are
any, its `compartments`:

```yaml
classification:
  level: confidential
  compartments: [hr]
```

### Compartments are configured

Which compartments exist, and which may read which, is configured by the
administrator in the top-level section `compartments` of the configuration
(`Compartments`, see [CONFIGURATION.md](CONFIGURATION.md)):

```yaml
compartments:
  - name: management
    reads: [hr]
  - name: hr
    reads: [finance]
  - name: finance
```

Every compartment reads itself. The read relation is **closed transitively**
when the configuration is read: what HR wrote may contain what HR read from
finance, so management, which reads HR, reads finance too. A compartment
that reads one that is not defined, or is defined twice, is a configuration
error.

### Channels

Every channel has a classification (`Channel::classification`), set by its
owner when creating it, in the session template or the request; the default
is `internal` without compartments (`Classification::CHANNEL_DEFAULT`).
Giving a channel a classification is asked as `ActionType::Classify`, before
anything is stored; the ownership policy refuses a compartment that is not
configured, so that a typo does not go unnoticed. The rules see a channel as
a `ChannelClassification`: its session, its id and its classification.

### Messages

Every message carries a classification (`Message::classification`, issue
133). It is not optional, and a `Message` has no default: a message is
built with `Message::new(classification)`, so that no code path can leave a
message classified by accident.

*   **Its sender classifies it.** An agent's messages - its answers, tool
    results, what its model is given - carry the classification its turn
    began with (`AgentActor::label` for everything its model produces; until
    then an `LLMClient` builds on `llm::unlabelled_message`, the strictest
    general classification). A user's message carries the channel's, unless
    the user classifies it otherwise.
*   **The channel admits it.** Putting a message into a channel is
    `ActionType::Publish { classification, actor, channel }`, asked by the
    `ChannelLoop` for every message of every event that enters it
    (`ChannelLoop::admits`) - the gate for every actor. An event with a
    message it may not admit is dropped and logged. The ownership policy
    allows it if the actor is the one acting (the context's caller), every
    compartment is configured, and the classification flows into the
    channel.
*   **Released when written.** By the flow rule a message is never
    classified above its channel. One classified otherwise is released: at a
    lower level, or to further compartments besides those the channel's are
    covered by - an `{hr, finance}` message in an `{hr}` channel is readable
    from finance from the start. A user may release what they write
    (`UserActor::send_message` asks `Publish` before it sends, so that a
    refusal reaches the user as `ChannelError::ClassificationRefused`).
    Changing a classification afterwards is
    [reclassifying](#reclassification) it.

### Reclassification

The classification of a channel or a message can be changed afterwards
(`ActionType::Reclassify { target, current, new, channel, attributes }`,
issue 133). Lowering it - a lower level, a compartment added to the
audience, or none left - *releases* what it holds to a wider circle; raising
it retracts nothing already read. The ownership policy allows a channel's
owner to reclassify the channel and every message in it, and a message's
author to reclassify their own message - a message never above its
channel, and only with configured compartments.

*   **A channel** (`ChannelLoop::reclassify`, REST `PUT
    …/channels/{cid}/classification`) is stored (`UpdateChannel`) and
    announced as `GlobalEvent::ChannelUpdated` with
    `ChannelUpdateEvent::ClassificationChanged`. Lowering it releases what
    is in it: every message is fitted into the new classification
    (`Classification::fitted_into`: its level at most the channel's, the
    channel's compartments added to its audience, none if the channel is
    general), since the agents in the channel carry what they know into
    what they write next. Messages released further before keep their
    wider audience. A turn that is running keeps the classification it
    began with until it ends.
*   **A message** (`ChannelLoop::reclassify_message`, REST `PUT
    …/messages/{mid}/classification`) is changed in place in the history -
    its digest does not cover its classification, so the record stays what
    it was - and announced as `GlobalEvent::MessageReclassified`, without
    content.

*   **A note or a memory entry** is reclassified by its agent, through a
    tool that asks for approval (`reclassify_note`,
    `reclassify_private_note`, `reclassify_memory`): a change of its chain
    (`NoteChange::Reclassify`, `MemoryChange::Reclassify`), taken over with
    the turn. The policy allows it to the author - the agent calling -
    never above the turn's channel; a memory entry only where it may be
    recalled. A reclassified memory entry is recalled where its new
    classification may be read. Agents do not reclassify channels or
    messages: people do.

Who reclassified what, and when, is not recorded: that is an audit log
([140](../issues/140-audit-log.md)); the chains of notes and memory keep
the change, but not who made it or who approved it.

## The flow rule

One relation decides everything (`Classification::flows_to`): information
classified S may flow into a destination classified D if

*   D's level is at least S's, and
*   S has no compartments, or D has at least one and every compartment of D
    is covered by one of S's - the same, or one it may read.

For a destination with one compartment this reads "any of the source's
compartments will do"; for a channel with two, what flows into it must be
meant for both, since both audiences see it. What is general flows into any
destination of a sufficient level; what has an audience never flows into a
general one.

## Reading another channel

Reading from another channel is decided per entry: per message of its
history, per note, per memory entry. It is `ActionType::ReadChannel {
source, classification, reader }`, with both channels'
`ChannelClassification` and the classification of the entry
(`AccessQuestions::may_channel_read`). It is asked in the name of the agent
that reads - the security context it acts in, its owner's - because the agent
passes on what it reads (issue 131). A channel may always read itself.
Otherwise the ownership policy allows it if

*   the entry's classification flows into the reader's, and
*   both channels belong to the same session, or the source is `public`.

So a channel reads the entries of another that were released to it, even
where it may not read that channel as a whole. Whether it may, is the
question asked with the source channel's own classification: a channel is
listed among the readable ones (`SessionReader::readable_channels`) only
then, and one that may not be read as a whole and of which nothing was
released reads as absent, so that channels cannot be probed.

The second condition keeps sessions apart until reading across sessions is a
question of the participants' consent
([134](../issues/134-session-privacy.md)): with classifications alone, two
`internal` channels of different sessions could read each other.

How an agent reads other channels - the `SessionReader`, `read_notes`,
`read_history` - is described in [MEMORY.md](MEMORY.md).

## What only the owner sees

Beside the classification there is a second axis: whether anyone but the
agent's owner sees something at all. That is ownership, not sensitivity,
and it is a flag of its own (`Visibility`): `channel` - published to the
channel - or `owner` - never published; the agent sees it, and its owner in
the context panel.

### Tool calls

A `Tool` declares its `ToolVisibility` (issues 54, 133): two flags for
its calls - `call`, whether the call itself - that the tool was called - is
published, and `content`, whether its arguments and its result are.

| Constant | `call` | `content` | Meaning |
|---|---|---|---|
| `SHOWN` | channel | channel | the default |
| `CALL_ONLY` | channel | owner | others see that the tool was called, but not with what |
| `HIDDEN` | owner | owner | the whole call stays with the agent |

The agent stamps every call with the flags its tool declares at that moment,
and the result takes them over from the call. Both are stored with what
applied then. Which built-in tools use which flags is listed in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

### Tool results

A tool result carries a classification of its own
(`ToolCallResult::classification`, issue 133): a tool classifies each of
its results by what it read or the source it drew on (`ToolOutput`), a tool
can be configured with one for all of its results (`Tool::classification`,
e.g. the `classification` of an `mcp_tool`), and otherwise a result is the
channel's. The `GuardedToolExecutor` applies them in this order.

**Before the model, not after it.** A result that may not flow into the
channel - asked as `ActionType::Publish`, in the agent's name, since the
result enters its context in the channel - is withheld from the agent: the
model is given the status `withheld` and a message that names the
classification instead, and the value is kept nowhere. So the answer text,
which carries no label, cannot quote it. A result that may flow keeps its
classification: below the channel's, it is released.

The channel admits a message only if its tool results may enter it too
(`ChannelLoop::admits`). Another channel reads a message by the message's
classification; a released result in a message it may not read stays
unread.

### Reasoning

`Message::reasoning_visibility` says who sees a message's reasoning. It is
`owner` if

*   the personality's `reasoning_visibility` is `owner` (default `channel`),
*   the turn has seen the agent's private knowledge - its private notes or
    its agent memory (`Turn::saw_private_knowledge`), or
*   a call the reasoning leads to has `owner` content: of the message's own
    calls once it is complete, and, while it streams, of every tool the
    agent could still call - because reasoning comes before the call.

Otherwise the reasoning is published and carries the channel's
classification, as everything the agent derives does. Knowledge read from
other channels adds nothing: a channel reads only what may flow into it.

The answer text itself carries no flag.

What a model is given of reasoning is a separate question from who may
read it: no model is given another actor's reasoning, and of the agent's
own only what its model's rule keeps (issue 63, see
[AGENT-TURN.md](AGENT-TURN.md#reasoning-given-back)).

### Redaction

The agent keeps the full calls in its context. What it publishes is redacted
in one place, `Turn::send_event` (`Message::published`):

*   a call whose `call` is the owner's is left out;
*   a call whose `content` is the owner's has its arguments set to `null` and
    its result emptied;
*   reasoning that is the owner's is dropped;
*   a message with nothing left is not sent.

The channel history is built from these events. History, REST, the event
stream, `read_history` and the moderator therefore only ever see the published
form. The context panel (`ReadAgentContext`) shows the agent's own, full view.

Approval requests are not redacted: the approvers see the arguments of a call
they are asked to decide.

### What the model is told

Since the answer text is always published, the model is told what it must not
quote (issue 66):

*   A successful result whose content is the owner's reaches the model
    wrapped as `{"result": ..., "note": "Only you see this result ..."}`
    (`tool::successful_result`).
*   The private notes' prompt part and tool descriptions say not to reveal
    them.

## Knowledge that carries its channel

What an agent writes down carries the classification of the channel it was
written in, stamped when it is written, so that the reading rule can follow
it into other channels; a later change of the channel changes none of it:

*   **Notes** (issue 17) and **private notes** (issue 51) carry their
    classification (`NoteChange::Note`, `Note::classification`). Other
    channels read them note by note; the agent is shown the private notes
    of every other channel of the session that its current channel may
    read (`AccessQuestions::may_channel_read`).
*   **Agent memory** (issue 62) entries carry the classification of the
    channel they were written in, and are recalled only in channels that may
    read it (`AgentMemory::recalled`, which asks
    `AccessQuestions::may_channel_read` like the private notes, issue 131).
    The reading channel is judged by the classification the turn began
    with.

This is sound because an agent never sees anything that may not flow into
its channel: everything it wrote may be read by the channel's audience and
no wider.

"Private" means private from other agents and other users, not from the
agent's owner: the owner sees private notes in the context panel, which is
theirs alone (issue 54).

## Open questions

The classification model is introduced in steps
([133](../issues/133-classification-model.md)); steps 1 to 4 are done.
Still to come:

*   **Sources that classify.** No built-in tool classifies its results, and
    an MCP server cannot yet say how its results are classified, only be
    configured with one classification for all of them
    ([139](../issues/139-mcp-result-classification.md)). Configuring a tool
    with a classification is trusted: it is not asked as `Classify`.
*   **A turn during a reclassification.** A turn keeps the classification
    it began with: its messages, after the channel was lowered, are
    classified above the channel and refused by it
    ([141](../issues/141-turn-during-reclassification.md)).
*   **Clearances.** Who may join a channel is asked as
    `ActionType::JoinChannel` with the channel's classification
    ([46](../issues/46-channel-participants.md)). The ownership policy
    allows every join: clearances of users and agents are not modelled
    (step 5). Yet the guarantees above are only as good as the control over
    who joins.
*   **Sessions.** Reading across sessions is refused except for `public`
    channels until it becomes a question of consent
    ([134](../issues/134-session-privacy.md)).
*   **Where the rules live.** The flow rule is applied by the ownership
    policy; any other policy would have to apply it too
    ([138](../issues/138-confidentiality-policy-in-ownership.md)).
*   **The answer text.** It carries no label and is always published; the
    model is only told what not to quote
    ([66](../issues/66-channel-level-in-prompt.md)). Keeping private knowledge
    out of it relies on the model.
*   **Whose rights.** Reading is asked in the name of the agent's owner. How
    an agent's rights combine with those of the user it acts for is open
    ([12](../issues/12-user-and-agent-permissions.md)).
*   **Knowledge beyond the session.** Knowledge kept across sessions or shared
    between agents ([18](../issues/18-knowledge-sharing.md)) and retrieval of
    notes ([59](../issues/59-retrieval-of-notes.md)) will have to carry and
    respect the classification of where it came from; a search index must not
    become a way around the flow rule.
*   **One enforcement for component state.** When the state of notes becomes
    generic component state ([65](../issues/65-generic-component-state.md)),
    the rules are to be applied by the core, not re-implemented per component.
    Today every component that reads across channels asks
    `AccessQuestions::may_channel_read` itself (issue 131).
