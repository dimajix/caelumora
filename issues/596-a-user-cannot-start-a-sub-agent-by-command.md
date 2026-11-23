# 596 — A user cannot start a sub-agent by command

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** Only an agent starts a sub-agent, by the tools of `caelumora-subagents` (issue 558); a user cannot have an agent start one by a command of the agent, though 558 asked for a command.

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

## Decision

The maintainer's decision of 2026-11-23, on the author's assessment:

- **A command of an agent.** A user asks an agent taking part in the thread to start a sub-agent -
  as `/subtask @bob <personality> <brief>`, its name not decided -, carried out by the agent in a
  turn of its own: the agent delegates as with `start_subagent`, the answer is given back to it,
  and it responds where the command was issued. It is offered by the component `subagents` beside
  its tools, with the same list of personalities, as `skills` offers `/skill` beside `load_skill`.
- **No command of the channel.** A sub-agent keeps the context of whom delegated small and gives
  its result back into it; a user has no such context. What is given back is published to the
  users anyway (`GivenToRead`), so a user given the answer would see a second time what the
  sub-agent's thread holds; its closed circle keeps them from asking again by writing into it; and
  the one gain - the answer shown in the line - stands in for a view of threads
  ([595](595-no-frontend-shows-another-thread-of-a-channel.md)).
- **Working apart with an agent**, reading along and asking again, is no delegation: it is a
  thread with that agent in which the user writes themselves. Moved to 595.

What *Done when* asks of a command now holds for the command of an agent alone.

### Still open for the command

- **Where its thread hangs.** A thread hangs at a message of another thread (`ThreadOrigin`); the
  tool's at the message carrying its call, but a command is no message.
- **Approval and who may issue it.** The tool asks for approval as its component is configured
  (`requires_approval`); a command asks none, and a command of an agent is granted to whoever may
  write in the channel - so another user could have an agent of someone else start a sub-agent
  owned by that agent's owner, as a mention asking for the tool can today. Whether the command is
  for the agent's owners alone.
- **Its name and arguments.**

## Done when

- It is decided which command starts a sub-agent and to whom its answer goes, and it is
  implemented, with its permission checks in PERMISSION-CHECKS.md.

## History

- **2026-11-18** — Opened from issue 558's open points (*The command*).
- **2026-11-23** — Narrowed by the maintainer to a command of an agent (*Decision*): the command of
  the channel is not pursued, and a user working apart with an agent in a thread is moved to 595.
