# 236 — A remote agent is not told when a message it was given is modified or taken back

**Status:** rejected
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
