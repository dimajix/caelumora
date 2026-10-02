# 146 — The web UI does not show the turns an agent gave up

**Status:** open
**Area:** User interaction

## The problem

A turn that fails or is stopped is given up ([145](145-ended-turns-leave-a-record.md)): its messages
stay in the agent's transcript as a branch no longer active, marked with why and when it ended, and
the REST server lists them (`GET …/actors/{aid}/given-up-turns`). The web UI shows none of it. A
user sees a `TurnFailed` notice, but not what the turn had done - which tools it called, with what,
and what they returned - although those effects remain.

## Why it matters

- The record of a given-up turn is there to be audited; one that can be read only through the API is
  audited by nobody.
- After a failed turn, the user is the one to decide whether its effects need attention; the UI
  gives them nothing to decide on.

## Considerations

- Where: in the context panel of the agent (issue [41](41-show-message-context.md)), as a list of
  given-up turns, or at the place in the chat where the turn failed. The chat shows the channel's
  history, which holds nothing of a given-up turn, so the place would have to be derived (the time,
  or the message the turn answered).
- Who: offered only where `can_read_context` holds for the agent, like the context of a message.
- Each message of such a turn has a context like any other, which the existing context view can
  show.

## Done when

- A user who may read an agent's context can see the turns it gave up in a channel, with their
  reason, messages and tool calls.

## History

- **2026-09-30** — Opened while implementing [145](145-ended-turns-leave-a-record.md): the
  maintainer wanted the REST endpoint there and the web UI as an issue of its own.
