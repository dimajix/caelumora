# 239 — A reclassified message keeps its old classification in every agent's state

**Status:** done
**Area:** Sessions and channels
**Summary:** A reclassified message keeps its old classification in every agent's state, so a released message still raises the label of later answers.

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

## Plan

A reclassification of a message is a change to what was said, made by a participant, and goes the
way [234](234-modify-revoke-and-retry-messages.md) built for modifications and revocations: an
`ActorEvent` from the actor of whoever makes it, admitted by the channel, recorded in its history
and routed to everyone who may hold the message, each of whom records it where it holds the message.

### What was weighed and not chosen

The reclassification stays an operation on the channel (`ChannelLoop::reclassify_message`), and
`run` announces it in the caller's name, as it announces `ParticipantRemoved` after
`remove_participant`. That keeps the channel's owner able to reclassify a message without being a
participant, but makes a second way for a change to what was said next to 234's: a variant of
`ActorChange` that is not about an actor, the history changed either in `run` or twice, and a
change that waits while the channel does not run. The owner who releases a message in a channel
they do not take part in is rare enough not to be worth it; they are refused as they are when
writing or approving (`not_a_participant`).

### The event

`ActorEvent::MessageReclassified { message: MessageId, classification: Classification }`: no
content, as the `GlobalEvent` it replaces. Sent by the actor of whoever reclassifies, in their
name - the author's own, or the channel's owner's for someone else's message.

### The model (`caelumora-model`)

- `Message::reclassify(&mut self, Classification) -> bool`: the rule, next to `modify` and
  `revoke` - only a message not taken back. A message taken back says nothing any more, so there is
  nothing to release.
- `MessageHistory::reclassify` asks it of the message as it stands (`false` for one taken back, as
  for one never said), and `fold` applies it through it.
- `TranscriptEntry::Reclassified { message, classification }`, applied by `as_they_stand` like
  the other changes (`changed`, `apply_to`), recorded by `Transcript::reclassify` for a message on
  the active path as it stands. Perceive processors do not run again: what they passed is the
  content, which does not change.
- `AgentChannelState::reclassify_message(id, classification) -> bool`: into the history and the
  transcript, where they hold it, as `revoke_message`. A turn begun afterwards has seen the
  message as it stands, so its label follows (`begin_turn`); `retried` skips the entry like the
  other changes.
- `digest::message_reclassified`, shared by both chains as `message_modified` is - in the scheme
  the history's entry has (`caelumora.history-reclassified.v1`), so that every digest recorded
  names what it named.

### The channel, routing and the agents (`caelumora-runtime`)

- `admits_change` admits the event if the message is in the history, may be reclassified so by the
  sender (`ActionType::Reclassify` with `Reclassified::Message`, in the envelope's context, never
  above the channel) and can be (`Message::reclassify`). `append_history` records it.
- `PolicyModerator` routes it with `MessageModified` and `MessageRevoked`
  (`route_to_participants`); `take_in` applies it for both kinds of agent
  (`AgentChannelState::reclassify_message`), between turns, as every change.
- `UserActor::reclassify_message(message, classification)` asks first (so that the user learns of
  a refusal), then sends it. `ChannelLoop::reclassify_message` and
  `GlobalEvent::MessageReclassified` are gone.

### REST, web UI, TUI

- `PUT …/messages/{mid}/classification` goes through the user's actor (`user_actor_of`), as the
  other changes to a message: same path, same body; a non-participant is answered `403
  not_a_participant`, a message taken back `409 message_revoked`.
- The web UI takes the classification from the channel's stream (`MessageReclassified`) instead of
  the global one. The TUI has no reclassification; it ignores the event.

### What is left out

- Lowering a channel (`HistoryEntry::Fitted`) stays an operation on the channel and reaches no
  agent's state: what an agent's history shows differs from the channel's, but its labels never
  exceed the channel's.
- A reclassification an agent's inbox drops is lost for it, as every change is: catching up finds
  missing messages, not missed changes ([223](223-full-inbox-drops-events.md)).

### Steps

1. **The model.** `Message::reclassify`, the history through it, the transcript entry with
   `Transcript::reclassify`, `AgentChannelState::reclassify_message`, the shared digest; the
   channel refuses a message taken back (`MessageRevoked`). Tests: a message taken back is not
   reclassified; the transcript gives the message as it stands, a digest from before names it as
   it was; a turn begun after a release is labelled as the message now is; a change reaches a
   stored chain. AGENT-TURN.md.
2. **The runtime.** The event, the gate, the routing, `take_in`,
   `UserActor::reclassify_message`; `ChannelLoop::reclassify_message` and the `GlobalEvent` go.
   Tests in `channel.rs`: the reclassification reaches every agent that holds the message, the
   owner's of another's message too, nobody else's; the security suite. CONFIDENTIALITY.md,
   CHANNELS.md, EVENTS.md.
3. **For users, and close.** The REST route through the user's actor, the web UI from the
   channel's stream; verified in the browser. REST-SERVER.md, WEB-UI.md; status `done`.

## Done when

A reclassification of a message reaches every agent that holds it, and the labels of later turns
follow from the message as it stands.

## History

- **2026-10-06** — Opened from *Things to watch* of [234](234-modify-revoke-and-retry-messages.md),
  which named it while building the path for modifications and revocations.
- **2026-10-06** — Decided with the maintainer: a reclassification goes through the actor of
  whoever makes it, as a modification does (the plan above); the channel's owner reclassifies
  another's message as a participant. Written against the code as of `89390f9`.
- **2026-10-06** — Step 1: `Message::reclassify` is the rule (not for a message taken back), the
  history reclassifies through it, the transcript records `TranscriptEntry::Reclassified`, read by
  `as_they_stand`, and `AgentChannelState::reclassify_message` applies it to both; both chains
  share `digest::message_reclassified`, in the history's scheme. Until step 2 the channel answers
  a reclassification of a message taken back with `MessageRevoked`.
- **2026-10-06** — Step 2: `ActorEvent::MessageReclassified` from `UserActor::reclassify_message`,
  admitted by `admits_change` with the new `AccessQuestions::may_reclassify`, recorded by
  `append_history`, routed with the other changes and taken in by `take_in` for both kinds of
  agent. `ChannelLoop::reclassify_message`, `GlobalEvent::MessageReclassified` and the channel's
  event bus, which only it used, are gone. The REST route had to go through the user's actor in
  this step already, since the channel's method is gone; the web UI follows in step 3 and until
  then shows a reclassification only once the channel is loaded again. The stand-in model of the
  channel's tests gave every answer the same id - unlike every model client - which a
  reclassification of one of them showed; it gives each its own now.
- **2026-10-06** — Step 3, done: the web UI takes a message's classification from
  `MessageReclassified` on the channel's stream; the route is described in REST-SERVER.md, the
  web UI in WEB-UI.md. Verified in the browser and over REST with a model (Ollama): the channel's
  owner released another's answer, shown at once as released and recorded in the agent's history;
  in a fresh channel, once the earlier question and answers were released, the agent's next answer
  to a public question was labelled public - before, its transcript would have kept it internal. A
  message taken back is refused with `409 message_revoked`, a classification above the channel with
  `403`.
