# 212 — An external agent's task that began before its channel was lowered is answered into the lowered channel

**Status:** open
**Area:** Confidentiality, integrations

## The problem

A turn of an agent run here keeps the channel's classification as it was when the turn began
(`TurnState::classification`). Before every step the `AgentActor` asks whether what the turn saw
may still flow into the channel as it is now (`turn_flow`); once the channel was lowered, the turn
is given up, nothing of it is published, and the users are told
([141](141-turn-during-reclassification.md)).

An external agent has no such check. Its `OpenTask` keeps no classification, and what comes back
is classified when it is published - as the remote agent's component declares, else as the channel
is *then* (`ExternalAgentActor::classification_in`). So:

1. The remote agent is given what the channel said while it was classified, say, `confidential`.
2. The channel is lowered to `internal` while the task works - for minutes, or across a restart
   as a followed task (`follow_open_tasks`).
3. The answer comes, is classified `internal` - the channel as it is now - and is published.

The answer was produced from what was `confidential`, and is shown to everyone who reads the
lowered channel. The same holds for the pieces published while an answer is written
(`PartialMessageGenerated`), and for a task that asks back.

A declared classification does not close it: it says what the remote agent produces on its own,
and is checked against the channel once, when the actor is built - not what it was given here.

## Why it matters

It is the case of issue 141 on the one path that issue left out, since external agents came later
([179](179-external-agents-via-a2a.md)). Lowering a channel is the operation that promises its
new readers that nothing above the new level reaches them; a task may be open for long, so the
window is wider than a turn's.

## Considerations

**Where the classification is kept.** An `OpenTask` gets the channel's classification as it was
when the task began, as a `TurnState` has it. [211](211-turn-and-agent-work-structure.md), step 5,
aligns the `OpenTask` with `TurnState` and is where the field fits; this issue adds it if it comes
first. A request answered without a task (the remote agent answers with a message at once) has no
`OpenTask`: the classification is then that of the exchange, held by the actor while it answers.

**What is done when the channel was lowered meanwhile.** Options:

1. *As a turn (recommended):* before anything of the task is published - a piece, the answer, a
   question back - ask `may_publish` for the classification it began with against the channel as
   it is now. If it may not: publish nothing, cancel the task at the remote agent, forget it, and
   tell the users with `TurnFailed` that the answer was discarded because the channel was lowered,
   in the words issue 141 uses. One rule for both kinds of agent.
2. *Publish it classified as it began.* The channel drops what is classified above it - in
   silence, which is the state issue 141 started from.
3. *Leave the task and publish when it ends, if the channel was raised again by then.* Keeps an
   answer nobody may see for an unknown time; no counterpart for agents run here.

**What the remote agent already heard.** The remote conversation keeps what it was given before
the lowering, and later tasks in it may tell of it. An agent run here keeps the same in its
transcript; how a lowered channel treats that is [67](67-change-channel-level.md)'s and
[141](141-turn-during-reclassification.md)'s answer, to be read first. If it is a gap there too,
it is one for both kinds and an issue of its own; if the transcript is cut or relabelled, the
external agent needs the counterpart - at the least a new remote conversation
(`RemoteState::conversation` forgotten) after a lowering.

**What is recorded.** A turn given up stays as a branch of the transcript. An external agent has
nothing to keep of an answer discarded; whether its history notes that an answer was discarded is
to be decided with [205](205-external-agent-message-context.md).

## Done when

- An open task keeps the channel's classification as it was when it began.
- An answer, a piece of one, or a question back of a task whose classification may no longer flow
  into its channel is not published; the task is cancelled at the remote agent, and the users are
  told as for a turn.
- The same holds for a task followed after a restart.
- A test in the security suite covers both: answered while asked, and followed.
- What the remote conversation heard before the lowering is decided - here, or in an issue this
  one names.
- CONFIDENTIALITY.md says that the rule for a turn holds for an external agent's task.

## History

- **2026-10-03** — Issue created. Found while reading `OpenTask` against `TurnState` for
  [211](211-turn-and-agent-work-structure.md): the `OpenTask` keeps neither whose authority it
  acts with nor the classification its channel had when it began. Read from the code, not yet
  reproduced in a test.
