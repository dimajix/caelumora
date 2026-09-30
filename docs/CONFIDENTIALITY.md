# Confidentiality

This document describes how far information may travel in Caelumora: the confidentiality levels of channels, the labels on the parts of a message, and the two rules that decide what may be published to a channel and what one channel may read of another. Who may do what is a separate question, answered in [SECURITY.md](SECURITY.md); the notes and memory these rules apply to are described in [MEMORY.md](MEMORY.md). The model is not finished - see [Open questions](#open-questions) at the end.

## Levels

What may travel from one channel to another is decided by confidentiality levels (`caelumora_model::confidentiality`, issue 61), not by who takes part in the channels. A channel is a construct of a certain trustworthiness, and the decision must not change when someone joins or leaves.

A level (`Confidentiality`) is a reach. The levels are ordered, from the least to the most confidential:

| Level | Reach |
|---|---|
| `public` | anywhere, beyond the session too |
| `session` | the channels of the same session |
| `channel` | only the one channel |
| `private` | never beyond the actor |

Every channel has one of the first three (`Channel::confidentiality`, default `session`). Its owner sets it when creating the channel, in the session template or the request; `private` is refused, because it is an actor's level, not a channel's. As the level of a channel, a level says how far the channel's contents may travel. The rules see a channel as a `ChannelClassification`: its session, its id and its level.

## Labels

Labels on the parts of a message say how far each part may travel. There are two kinds.

### Tool calls

A `Tool` declares its `ToolConfidentiality` (issues 54, 61): two labels for its calls - `call`, how far the call itself may travel, and `content`, how far its arguments and its result may.

| Constant | `call` | `content` | Meaning |
|---|---|---|---|
| `PUBLIC` | public | public | the default |
| `CALL_ONLY` | public | private | others see that the tool was called, but not with what |
| `HIDDEN` | private | private | the whole call stays with the agent |

The agent stamps every call with the labels its tool declares at that moment, and the result takes them over from the call. Both are stored with what applied then. Which built-in tools use which labels is listed in [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

### Reasoning

`Message::reasoning_confidentiality` labels a message's reasoning. The agent sets it to the most confidential of:

*   the personality's `reasoning_confidentiality` (default `public`);
*   `private`, once the turn has seen the agent's private knowledge - its private notes or its agent memory (`Turn::saw_private_knowledge`);
*   the content labels of the calls the reasoning leads to: of the message's own calls once it is complete, and, while it streams, of every tool the agent could still call - because reasoning comes before the call.

Knowledge read from other channels adds nothing: the [reading rule](#reading-another-channel) lets a channel read only what reaches no further than its own contents.

The answer text itself is not labelled.

## Publishing to a channel

Publishing a labelled part to a channel is `ActionType::Publish { label, actor, channel }`. The ownership policy allows it if the channel admits the label (`ChannelClassification::admits`): if the label reaches no further than the channel's contents may travel anyway. This is rule 1 of issue 61.

*   **Once per turn.** The agent asks once per turn and level, in its owner's name, when the turn begins, and keeps the answer as the turn's `Publication`. Without a decision, only `public` is published.
*   **Never `private`.** `private` is never published, whatever a policy says; it is not even asked for.
*   **Stable.** The decision depends only on the label, the actor and the channel, so it holds for everyone who joins later.

### Redaction

The agent keeps the full calls in its context. What it publishes is redacted in one place, `Turn::send_event`, by the turn's `Publication`:

*   a call whose `call` label is not admitted is left out;
*   a call whose `content` label is not admitted has its arguments set to `null` and its result emptied;
*   a message with nothing left is not sent.

The channel history is built from these events. History, REST, the event stream, `read_history` and the moderator therefore only ever see the published form. The context panel (`ReadAgentContext`) shows the agent's own, full view.

Approval requests are not redacted: the approvers see the arguments of a call they are asked to decide.

### What the model is told

Since the answer text is always published, the model is told what it must not quote (issue 66):

*   A successful result whose content label the turn may not publish reaches the model wrapped as `{"result": ..., "note": "Only you see this result ..."}` (`tool::successful_result`).
*   The private notes' prompt part and tool descriptions say not to reveal them.

## Reading another channel

Reading another channel's notes and history is `ActionType::ReadChannel { source, reader }`, with both channels' `ChannelClassification`, asked in the name of the reading channel's owner. The ownership policy allows it if (`ChannelClassification::readable_from`, rule 2 of issue 61):

*   the reader lies within the source's reach, and
*   the reader's level is at least the source's.

The second condition exists because an agent passes on what it reads to its channel. So a `public` channel reads only public contents, and a `channel`-level channel is read by no other. A channel may always read itself.

How an agent reads other channels - the `SessionReader`, `read_notes`, `read_history` - is described in [MEMORY.md](MEMORY.md).

## Knowledge that carries its channel

What an agent keeps privately is labelled with the channel it was written in, so that the reading rule can follow it into other channels:

*   **Private notes** (issue 51) carry the confidentiality of the channel they were written in. The agent is shown the private notes of every other channel of the session that its current channel may read (`AccessQuestions::may_channel_read`), checked against that channel's current level at every read.
*   **Agent memory** (issue 62) entries carry the classification of the channel they were written in, and are recalled only in channels that may read it (`AgentMemory::recalled`, by `ChannelClassification::readable_from`).

"Private" means private from other agents and other users, not from the agent's owner: the owner sees private notes in the context panel, which is theirs alone (issue 54).

## Open questions

The model is a first step. Not decided yet:

*   **Compartments.** Levels are only ordered; information that may be shared between the channels of one topic or group, but not with others, cannot be expressed ([64](../issues/64-confidentiality-compartments.md)).
*   **Clearances.** Who may join a channel is asked as `ActionType::JoinChannel` with the channel's classification ([46](../issues/46-channel-participants.md)), so that a policy can hold the channel's level against the newcomer. The ownership policy allows every join: clearances of users and agents are not modelled. Yet the guarantees above are only as good as the control over who joins.
*   **Changing a channel's level.** A level can only be set when the channel is created ([67](../issues/67-change-channel-level.md)). Reading would follow a change at once; a running turn keeps its `Publication` until it ends.
*   **The answer text.** It carries no label and is always published; the model is only told what not to quote ([66](../issues/66-channel-level-in-prompt.md)). Keeping private knowledge out of it relies on the model.
*   **Whose rights.** Publishing and reading are asked in the name of the agent's or the channel's owner. How an agent's rights combine with those of the user it acts for is open ([12](../issues/12-user-and-agent-permissions.md)).
*   **Knowledge beyond the session.** Knowledge kept across sessions or shared between agents ([18](../issues/18-knowledge-sharing.md)) and retrieval of notes ([59](../issues/59-retrieval-of-notes.md)) will have to carry and respect the classification of where it came from; a search index must not become a way around the reading rule.
*   **One enforcement for component state.** When the state of notes becomes generic component state ([65](../issues/65-generic-component-state.md)), the rules are to be applied by the core, not re-implemented per component. Today agent memory filters by `ChannelClassification::readable_from` itself, while private notes and `read_notes` ask the policy through `AccessQuestions::may_channel_read`.
*   **One default level.** Six helper functions in five modules define the default confidentiality level ([97](../issues/97-small-model-unifications.md)).
