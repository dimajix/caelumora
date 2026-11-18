# 598 — A sub-agent nobody ends stays

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** A sub-agent of `caelumora-subagents` (issue 558) is ended only by the agent that started it (`end_subagent`); one it never ends stays - an agent, a participant of the session and an open thread - for as long as the session lasts, and across restarts.

## The problem

Issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md) left open (*An idle sub-agent*) whether a
sub-agent the main agent never ends is ended after a while. Today it is not: the agent stays in the
store, a participant of the session and its channel, its thread open, its delegate in the channel's
state; a model that forgets to call `end_subagent` leaves one behind with every task it hands off.

## Considerations

- **Ended after a while without a brief**, by the channel or the plugin - when, measured how, and
  whether a restart counts.
- **Ended with the main agent's turn, or its leave**, as a task of the work.
- **Ended with the session**, deleted as the session is.
- What an agent is told when its sub-agent was ended for it - so that `ask_subagent` answers
  `NoSubAgent` meaningfully.

## Done when

- It is decided when a sub-agent nobody ends is ended, and it is implemented and tested.

## History

- **2026-11-18** — Opened from issue 558's open points (*An idle sub-agent*).
