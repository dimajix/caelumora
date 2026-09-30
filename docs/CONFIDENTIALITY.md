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

Reading another channel's notes and history is `ActionType::ReadChannel {
source, reader }`, with both channels' `ChannelClassification`
(`AccessQuestions::may_channel_read`). It is asked in the name of the agent
that reads - the security context it acts in, its owner's - because the agent
passes on what it reads (issue 131). A channel may always read itself.
Otherwise the ownership policy allows it if

*   the source's classification flows into the reader's, and
*   both belong to the same session, or the source is `public`.

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

A `Tool` declares its `ToolConfidentiality` (issues 54, 133): two flags for
its calls - `call`, whether the call itself - that the tool was called - is
published, and `content`, whether its arguments and its result are.

| Constant | `call` | `content` | Meaning |
|---|---|---|---|
| `PUBLIC` | channel | channel | the default |
| `CALL_ONLY` | channel | owner | others see that the tool was called, but not with what |
| `HIDDEN` | owner | owner | the whole call stays with the agent |

The agent stamps every call with the flags its tool declares at that moment,
and the result takes them over from the call. Both are stored with what
applied then. Which built-in tools use which flags is listed in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

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

What an agent keeps privately carries the classification of the channel it
was written in, so that the reading rule can follow it into other channels:

*   **Private notes** (issue 51) carry the classification of the channel
    they were written in. The agent is shown the private notes of every
    other channel of the session that its current channel may read
    (`AccessQuestions::may_channel_read`), checked against that channel's
    current classification at every read.
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
([133](../issues/133-classification-model.md)); only the first is done.
Still to come:

*   **Messages, notes and memory entries.** Only channels carry a
    classification of their own yet; a message a user writes cannot be
    classified differently from its channel, and reading is decided per
    channel, not per message (step 2).
*   **Tool results.** A tool cannot classify its results one by one; every
    result carries the channel's classification. Results that may not flow
    into the channel are to be withheld from the model by the
    `GuardedToolExecutor` (step 3).
*   **Release.** An authorised actor cannot yet widen an entity's audience or
    lower its level (step 4), and a channel's classification can only be set
    when it is created ([67](../issues/67-change-channel-level.md)).
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
