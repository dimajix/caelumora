# 146 — The web UI does not show the turns an agent gave up

**Status:** done
**Area:** User interaction
**Summary:** The web UI does not show the turns an agent gave up, so what their tools did can be read only through the API.

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
- **2026-10-05** — Done in the history panel of [207](207-agent-history-in-ui.md), which did not
  exist when this was opened: it is the agent's own view, offered only where `can_read_context`
  holds, and a given-up turn is part of what the agent did. The context panel shows one message, the
  chat the channel's history - neither fits.

  The turns are shown where they branched off, as folded blocks with why and when they ended, and
  opened with their messages, tool calls and results, each with its context. The place comes from
  the server: `GivenUpTurn::after` is the last message before the turn that is not internal - one
  the agent's history holds too - so the client neither guesses from timestamps nor knows how the
  transcript branches. The endpoint's messages are now `HistoryMessageResponse`s, with `has_context`
  as in the history; that their contexts can be shown holds because `give_up_turn` keeps them in
  the history as a branch no longer active. A failed turn's notice in the chat offers "What the turn
  did", opening the panel with the latest turn given up unfolded.

  Not covered: a turn given up because the agent was removed cannot be shown once it has left, since
  the panel is offered for members only - noted in [164](164-soft-deletion-and-former-members.md).
- **2026-10-08** — Changed by [258](258-what-was-said-is-lost.md): a turn given up is no longer a
  branch of its own, so `GET …/given-up-turns` and the folded blocks are gone. Its messages stand in
  the agent's history - and in the channel's, so in the chat - followed by its end, which the
  history panel and the chat both show where it ended, with why. The chat's "What the turn did" went
  with it: what the turn did is in the chat.
