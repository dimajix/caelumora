# 207 — An agent's history cannot be shown

**Status:** open
**Area:** User interaction

## The problem

Of an agent's state in a channel, the web UI shows the context of a single message (`ContextPanel`,
issues 06 and 47) and, through the REST API, the turns it gave up (issue 145). The agent's
**history** - all that was offered to it, and what it said itself (`AgentChannelState::history`,
`ExternalAgentChannelState::history`, see [AGENT-TURN.md](../docs/AGENT-TURN.md)) - cannot be shown
at all: `AgentContextView` hands out the transcript but not the history, and no route returns
either.

Unlike a message's context, the history is not tied to a message: it is one sequence per agent and
channel, so it is shown for the agent as a whole.

## Why it matters

- The history is not the channel's: an agent that joined late, was asleep or caught up after a
  restart (issue 190) holds what it was offered, which may differ from what the channel shows. To
  understand why an agent answered as it did, one has to see what it had heard.
- It is where the history and the transcript part: what the perceive processors dropped or changed
  (issue 196) is visible only side by side with the transcript.
- For an external agent ([205](205-external-agent-message-context.md)) the history is the record
  of everything that could have been sent out.

## Considerations

- **Runtime:** `AgentContextView` gains `history()`, answered by both views (`InternalContext`,
  `ExternalContext`) from the `MessageHistory` both states hold - a case for
  [206](206-one-agent-channel-state.md), where it would be written once.
- **Permission:** the same as a message's context, `ReadAgentContext` (`MemberView::context()`).
  The history holds no prompt parts or tool results of the agent's own turns, only what was said,
  but it may hold what the viewer does not see in the channel any more; a weaker permission would
  need a reason.
- **REST API:** `GET /sessions/{id}/channels/{cid}/actors/{aid}/history`, next to
  `…/given-up-turns`. A history grows with the channel, so it is paged like the channel's history
  (the last messages, older ones on request).
- **Web UI:** opened from the member list (or the agent's name), not from a message, in a panel
  like `ContextPanel`. Worth considering: marking what the agent heard but did not pass on to its
  transcript, and the messages it produced linking to their context.
- **The transcript** could be shown the same way; whether that belongs here or is a later step is
  to be decided.

## Done when

- Whoever may read an agent's context can see its whole history in a channel, for agents run here
  and external agents alike.
- The REST API returns it paged, and the web UI shows it per agent.
- AGENT-TURN.md (*Asking what an agent saw*), REST-SERVER.md and WEB-UI.md say so.

## History

- **2026-10-03** — Issue created. The history is shown per agent, not per message.
