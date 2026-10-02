# 173 — Nothing can examine what a message says before the channel routes it

**Status:** open
**Area:** Sessions and channels

## The problem

What enters a channel is checked only by its classification: `ChannelLoop::admits` asks the policy
whether every part of a message may be published into the channel (`ActionType::Publish`). Nobody
looks at the content. There is no way to have a further model step classify a message, or redact or
refuse what may not be said in a channel, before the moderator routes it to the other participants.

Such a step has to apply to the messages of every participant of a channel - users and agents
alike - so it belongs to the channel, not to an agent. And it has to be configurable per channel, in
the session or channel template.

## Why it matters

- A sender classifies their own messages. A user who forgets to, or an agent whose turn label is
  wrong, puts content into a channel classified lower than it should be - released to a wider circle
  than intended. Nothing notices.
- Some channels need content rules that no classification expresses: no personal data, no customer
  names, nothing off-topic.
- Once a message has been routed, it cannot be taken back: the other participants have read it, and
  the agents have it in their transcripts.

## Considerations

### Where: before the moderator, in the channel

`ChannelLoop::admits` is already the one gate that every message of every actor passes before it is
routed. Inspection is a second, content-based check at the same place. Because it happens before the
message is stored, a refused or redacted original never enters the channel's history, so no deletion
of messages is needed (compare [164](164-soft-deletion-and-former-members.md),
[169](169-changing-classified-content.md)).

The moderator is the wrong place: it decides whom to ask, not what may be said
([174](174-moderator-as-component.md)). An actor like the approver is the wrong place too: actors
receive messages only after they were routed.

### One check, several outcomes

By the flow rule a message is never classified above its channel. So "classifying" and "censoring"
are not two mechanisms but outcomes of one inspection:

- **Admit** - unchanged.
- **Admit with a stricter classification** - the message was released (lower than the channel, or
  with a wider audience), but its content should not have been. Raised at most to the channel's
  classification.
- **Redact** - the content does not belong in this channel at any classification it can hold; a
  redacted version with the reason is admitted instead.
- **Reject** - nothing of it is admitted.

**Decided:** one component facet (working name `MessageInspector`) with this result type. Several
inspectors of one channel run one after another; the strictest outcome wins and each sees what the
previous one admitted.

### Streaming

`admits` checks `PartialMessageGenerated` and `IntermediatePartialGenerated` piece by piece: the
tokens of an answer reach the users before the whole message exists. An inspector can judge only the
whole message; redacting it afterwards is worthless, everybody has read it.

- **(a)** No streaming to others in a channel with inspectors: partial messages are held back until
  the complete message has been inspected.
- **(b)** Partial messages go only to their sender (and the agent's owner), the others see the
  inspected message.
- **(c)** Stream and withdraw afterwards - acceptable at most for classification, never for
  redaction.

**Decided:** (a) as the default; (b) as a later refinement.

### The channel loop must not wait for a model

A model call takes seconds. If `admits` waited for it inline, approvals, `TurnFailed`,
`ParticipantRemoved` and cancelling would wait behind it. The messages of the channel need an
ordered inspection queue while control events pass by - and it has to be worked out which events may
overtake a message under inspection and which may not (a `ResponseRequested` caused by an
uninspected message may not).

### Telling the sender, and who repairs what

The sender learns what happened to their message - an event to the sender, working name
`MessageModerated { id, outcome, reason }`.

- **An agent as sender** keeps its original answer in its transcript. If the channel stores another
  version, transcript and history diverge - the kind of divergence [76](76-channel-cancel-flag.md)
  removed. An agent cannot edit its transcript, and asking its model to would be unreliable.
  **Decided:** the runtime repairs it mechanically - the transcript takes the admitted version or
  the raised classification - and the agent is told, so that it does not refer to what nobody saw.
- **A user as sender** is only informed; the web UI shows their message as it was admitted, with the
  reason.
- **What the agent derived from it** in the same turn - notes, memory entries - is the hard part. If
  an inspector finds the content more sensitive than the turn's label, the high-water mark of
  [168](168-turn-label-as-high-water-mark.md) was wrong for everything that turn wrote. Left open
  here; it needs its own issue once the rest stands.

### Configured as components of the channel

Today channels own no components; agents do, and the moderator is a name built by
`RuntimeManager::new_moderator`. Inspectors are components of the channel, named in the
`ChannelTemplate` (and in a channel created on request), built through the `ComponentRegistry`, and
may refer to a model component as an agent does since [130](130-llm-configuration.md). This follows
the pattern of [127](127-tool-executor-not-replaceable.md): a provider facet, and the channel which
receives the ready providers and stands in front.

Components on a channel are introduced by [174](174-moderator-as-component.md) as well; whichever is
done first lays the ground (template and entity field, a `ComponentContext` for a channel without an
actor) for the other.

### Security

- **In whose name** an inspector raises, redacts or rejects: `System`, or an `ActorId` of its own.
  Who changed a message is a question for the audit log ([140](140-audit-log.md)).
- **The inspector's model reads every message of the channel.** An external model inspecting a
  `strictly_confidential` channel is an outflow. There has to be a rule which models may inspect
  which channels - presumably the same question for an agent's model.
- **Failure:** if the inspector's model is unreachable, the message is rejected (fail closed) and
  the sender told, as with `TurnFailed`. Configurable per inspector.
- **Scope:** tool results, intermediate messages and `FullResponseGenerated` pass `admits` as well.
  Everything that goes into the history is inspected.

## Done when

- A channel template can name inspectors, and a channel created on request can be given them.
- Every message that enters such a channel is inspected before it is routed, in order, without
  blocking control events.
- Admit, raise, redact and reject work as described; the history holds only what was admitted.
- Nothing of a message reaches other participants before it was inspected.
- The sender is told; an agent's transcript holds what was admitted.
- An unreachable inspector rejects, unless configured otherwise.
- [CHANNELS.md](../docs/CHANNELS.md), [CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md) and
  [COMPONENTS.md](../docs/COMPONENTS.md) describe it.

## History

- **2026-10-01** — Opened. Decided in the discussion: inspection in the channel before the
  moderator, one facet with the outcomes admit/raise/redact/reject, no streaming to others in an
  inspected channel, the runtime repairs an agent's transcript, inspectors as components of the
  channel. Open: derived notes and memory, which models may inspect which channels.
- **2026-10-01** — [174](174-moderator-as-component.md) gave channels their components
  (`Channel::components`, built by `ChannelLoop::new` in the name of the channel's owner);
  inspectors can be added to them.
