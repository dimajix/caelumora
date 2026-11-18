# 596 — A user cannot start a sub-agent by command

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** Only an agent starts a sub-agent, by the tools of `caelumora-subagents` (issue 558); a user cannot hand a task to one by a command, though 558 asked for it, since it is not decided to whom its answer is given and in whose name.

## The problem

Issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md) wanted a user to start a sub-agent too.
The crate `caelumora-subagents` gives an agent's work the `SubAgentWriter` and the tools
`start_subagent`, `ask_subagent` and `end_subagent`; a delegate gives the sub-agent its brief and
gives back what comes of it to whom delegated. For a user, 558 left open (*The command*):

- **A command of the channel**, issued by the user: the user is the one who delegates, and the
  answer comes back into the line as a message - to whom, and in whose name?
- **A command of an agent** (`/subtask @bob <task>`), carried out by the agent in a turn of its own,
  as `/compact` is: the agent delegates, as with the tool, and the answer comes back to it.

## Considerations

- A user as giver of a delegation: `GivenToRead` gives the answer to its recipient by name, which a
  user's actor would have to take in and show.
- The command of an agent reuses the tools' writer whole; the command of the channel needs the
  delegate to give back to a user, and the frontends to show it (issue
  [595](595-no-frontend-shows-another-thread-of-a-channel.md)).
- Ending: who ends a sub-agent a user started, and whether it ends with the user's session.

## Done when

- It is decided which command starts a sub-agent and to whom its answer goes, and it is
  implemented, with its permission checks in PERMISSION-CHECKS.md.

## History

- **2026-11-18** — Opened from issue 558's open points (*The command*).
