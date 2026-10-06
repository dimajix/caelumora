# 239 — A reclassified message keeps its old classification in every agent's state

**Status:** open
**Area:** Sessions & channels

## The problem

Reclassifying a message (`ChannelLoop::reclassify_message`, issue [133](133-classification-model.md))
changes it in the channel's history and announces `GlobalEvent::MessageReclassified` to the user
interfaces. No agent learns of it: every agent's history and transcript keep the message as it was
classified when it was heard. A turn's label is the join of what its transcript holds
(`AgentChannelState::begin_turn`, issue [168](168-turn-label-as-high-water-mark.md)), so a message
released to a wider audience keeps raising the label of every later answer of the agents that heard
it, as if it had not been released.

## Why it matters

Releasing a message is meant to let what is built on it travel further. As it is, the agents'
answers stay classified as high as before, and what an agent's history shows differs from the
channel's.

## Considerations

- [234](234-modify-revoke-and-retry-messages.md) built the path for exactly this kind of change: an
  `ActorEvent` from the actor that makes it, admitted by the channel and routed to everyone who may
  hold the message, recorded by each store that does (`HistoryEntry::Reclassified` exists already;
  the transcript would need its own entry, read by `transcript::as_they_stand`). A reclassification
  is made by the author or the channel's owner (`Reclassified::Message`), so the owner's would be an
  event in their name, not the author's.
- Lowering a channel fits its messages into it (`MessageHistory::fit_into`) in the channel's history
  alone, too; the agents' state keeps them as they were. That shows in an agent's history, but not
  in its labels, which never exceed the channel's.

## Done when

A reclassification of a message reaches every agent that holds it, and the labels of later turns
follow from the message as it stands.

## History

- **2026-10-06** — Opened from *Things to watch* of [234](234-modify-revoke-and-retry-messages.md),
  which named it while building the path for modifications and revocations.
