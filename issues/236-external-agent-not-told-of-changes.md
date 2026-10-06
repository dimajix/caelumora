# 236 — A remote agent is not told when a message it was given is modified or taken back

**Status:** open
**Area:** Integrations

## The problem

An external agent ([179](179-external-agents-via-a2a.md)) keeps what it heard in its transcript
until it is next asked, then gives it to its remote agent, and its transcript starts anew
([205](205-external-agent-message-context.md)). Since [234](234-modify-revoke-and-retry-messages.md)
a user can modify or take back a message. The external agent records the change in its history,
and in its transcript as long as the remote agent was not given the message yet. Once it was, the
change reaches nobody: the remote agent keeps the old version and goes on answering to it.

## Why it matters

Correcting one's message is meant to stop the old version from acting. For an agent that runs
elsewhere it does not, and nothing tells the user so - in a channel with agents of both kinds the
same correction works for one and not for the other.

## Considerations

- The remote agent keeps its conversation; A2A has no way to change what was sent. What can be done
  is telling it, with the next call, as text: "alice corrected an earlier message: ...", "alice
  took back an earlier message". A prompt part of the external agent's personality, given like the
  roster (`changes_only`), would fit the pattern of [204](204-external-agent-roster.md).
- Taking back cannot be undone remotely; the text can only ask the remote agent to disregard it.
- Retry is rejected by external agents (`RetryRefused::Elsewhere`) for the same reason; whether a
  remote protocol can ever offer it is a question of its own.
- **What A2A 1.0 offers** (checked against the specification): no method changes, takes back or
  asks again for anything sent - the methods are `SendMessage`, `SendStreamingMessage`, `GetTask`,
  `ListTasks`, `CancelTask`, `SubscribeToTask`, the push configurations and
  `GetExtendedAgentCard`. A task that ended takes no further message; the conversation goes on
  with a new task in the same `contextId`. `referenceTaskIds` names earlier tasks, not messages.
  `metadata` and `extensions` could carry a change only by an extension both sides speak.
- **What Hermes offers** (its A2A plugin, `plugins/platforms/a2a/`): a `contextId` is a session of
  its gateway; of a message only the text of its parts is read - `metadata`, `extensions` and
  `referenceTaskIds` are not, and a `taskId` sent is not either: every message begins a task of its
  own. Every text that comes in is framed as untrusted input of a peer, `/`-text too, so that a
  peer never reaches the operator's commands: its `/retry` and `/undo` are deliberately out of
  reach. Its injection filter replaces, among others, `disregard (all|any|the) (previous|prior|
  above)` with `[filtered]` - the wording of a notice would matter. `CancelTask` drops the answer
  but does not stop the turn under way, and a context takes `A2A_MAX_PINGPONG_TURNS` calls (5 by
  default, 20 at the most) until it was idle for an hour or a task of it was cancelled; after that
  every call is rejected.
- **Retry over A2A** could only be had by a new `contextId` given the whole conversation as it
  stands - a conversation begun anew, in which the remote agent loses its own state while the old
  one stays with it, and Hermes's memory reaches across sessions all the same - or by asking in
  text, which leaves the old answer standing. `RetryRefused::Elsewhere` stays.

## Plan

The remote agent is told of a change to a message it was given as something said of its own, with
its next call, at its place among what was said since: "alice changed an earlier message ...",
"alice took back an earlier message ...". It is recorded in the external agent's transcript like
every change, so that what the remote agent was given can still be reconstructed
([205](205-external-agent-message-context.md)). A notice only asks: the remote agent - Hermes takes
it as untrusted input of a peer - decides whether the old version stops acting.

### What was weighed and not chosen

- **A prompt part** given like the roster ([204](204-external-agent-roster.md), `changes_only`): it
  would have to be configured on every external personality, and it stands before what was said,
  not at its place among it.
- **Working the notices out when the call is made**, from the history: nothing would record that
  the remote agent was told, and the context of what comes of the call would no longer say what it
  was given.
- **An extension of A2A, or `metadata`:** only a remote agent that speaks it would understand it;
  Hermes reads nothing but the text.
- **The transcript deciding alone** that a branch off its active path was given: it cannot tell a
  branch given to a remote agent from a turn given up or an answer asked for again.

### The model (`caelumora-model`)

- **`Transcript::tell(change)`** records a change - `TranscriptEntry::Modified` or `Revoked` - on
  the active path for a message that is not on it but stands on a branch off it, as the remote
  agent knows it: with every change recorded since, on whichever branch. `false`, and nothing is
  recorded, if no branch holds the message, or holds it taken back already. `modify` and `revoke`
  keep their rule.
- **`AgentChannelState::modify_message` and `revoke_message`** tell a change the transcript did not
  take on its active path, if the state keeps a remote conversation (`remote.conversation` is set):
  only then was the message given into a conversation that still holds it. A state of an agent run
  here never has one, so nothing changes for it. A modification of which nothing passes the
  perceive processors is told as taken back, as it is recorded today.
- **What is given next** comes from the transcript in order, as it stands: each message, and for
  each change told, the message as it was given before and as it stands now - a pair of messages,
  no type of its own (`Transcript::to_give() -> Vec<(Option<Message>, Message)>`, the first `None`
  for a message given for the first time). Several changes to one message before the next call are
  told once, as it stands then: modified twice, the last text; modified and taken back, taken back.
- **Reclassifying** is not told: the remote agent knows no labels.

### The external agent (`caelumora-runtime`)

- **`exchange`** gives the remote agent `to_give` instead of `messages`: a notice is a `Said` of the
  author of the change, worded in one place of the external agent's actor, independent of the
  protocol, in English, before its sender is marked as the profile says (`sender_format`):
  - `[alice changed an earlier message "<as it was given>"; it now reads:] <as it stands>`
  - `[alice took back an earlier message "<as it was given>"; it no longer stands.]`

  The text as it was given is cut to some 200 characters: it names the message, and was given
  already. No pronoun, and nothing a filter takes for an injection - Hermes's turns `disregard the
  previous` into `[filtered]`.
- **A notice is something given:** a request to respond after which only a notice is new still
  makes a call. A notice alone makes none; it waits for the next request.
- **The label of the call** sees what is given: the classification of a message as it stands, its
  modification and its reclassification included - today only the messages as they were first
  recorded are seen, so a message raised by a modification before it was given labels the answer
  too low. Seeing every change only raises the label, as the remote agent may have used either
  version.
- No new action acts in anyone's name: the notice is given in the call, under the request's
  security context, as what was said is.

### Documentation

CHANNELS.md (*Changing what was said*) and the passage on external agents in ARCHITECTURE.md say
that a remote agent is told of a change to a message it was given, as a notice with its next call,
and that it is asked, not made, to follow it.

### What is left out

- **Reclassifying** and **lowering a channel** are not told (see above).
- **Retry** stays rejected (`RetryRefused::Elsewhere`).
- **A remote agent that opened no conversation** is given the conversation anew each time and is
  told nothing.
- **Hermes's limit of calls per context** (`A2A_MAX_PINGPONG_TURNS`) is not dealt with here.

### Steps

1. **The model:** `Transcript::tell` and `to_give`, `AgentChannelState` telling a change of a given
   message, with tests: a change told on the active path, as it stands after several changes, not
   for a message no branch holds, not for a state without a remote conversation.
2. **The external agent:** `exchange` gives `to_give` with the notices worded, a notice counts as
   given, the label sees every change; tests in `external_agent.rs` and a channel test of a
   modification and a revocation reaching the remote agent; documentation; status done.

## Done when

A remote agent is told, with its next call, of every modification and revocation of a message it
was given, and the documentation says what that can and cannot achieve.

## History

- **2026-10-06** — Opened from open question 6 of [234](234-modify-revoke-and-retry-messages.md).
- **2026-10-06** — Rejected. A2A 1.0 and Hermes were checked (see *Considerations*): neither offers
  a way to change, take back or ask again for what was sent, and Hermes reads nothing but the text.
  A notice in text is all that could be done, and it would only ask - the remote agent, which takes
  it as untrusted input of a peer, decides whether the old version stops acting. A correction
  reaches a remote agent only as long as it was not given the message yet; CHANNELS.md says so, and
  why.
- **2026-10-06** — Reopened. Even if a notice can only ask, the remote agent should learn that a
  message it was given was changed or taken back - as something said of its own, given with the
  next call.
- **2026-10-06** — Planned (see *Plan*): a notice, said in the name of the change's author, recorded
  in the external agent's transcript and given with the next call.
