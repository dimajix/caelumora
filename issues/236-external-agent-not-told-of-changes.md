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

## Done when

A remote agent is told, with its next call, of every modification and revocation of a message it
was given, and the documentation says what that can and cannot achieve.

## History

- **2026-10-06** — Opened from open question 6 of [234](234-modify-revoke-and-retry-messages.md).
