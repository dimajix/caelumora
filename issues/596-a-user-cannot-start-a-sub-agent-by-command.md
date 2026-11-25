# 596 — A user cannot start a sub-agent by command

**Status:** done
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

### The command

What stayed open for the command, decided by the maintainer on 2026-11-25, on the author's
assessment:

- **What the agent is given.** A command turn leaves nothing in the agent's transcript but what
  the command gives it; without it, what the sub-agent gives back would come from nowhere, and the
  agent would know neither the brief nor the thread to ask or end it in. So the command gives the
  agent a message to read in the issuer's name, as `/skill` does (`Turn::give`): that it started a
  sub-agent of that personality on that brief, in which thread, and that what comes of it will be
  given to it there. Its text is in the prompt catalogue, under `subagents`.
- **Where its thread hangs: at that message.** A thread hangs at a message of another thread
  (`ThreadOrigin`), the tool's at the message carrying its call; a command is no message, but the
  message it gives the agent is, in the thread the command was issued in - where the delegate
  gives back what came of it, and where the agent, asked so, responds. Its id is chosen first, the
  sub-agent started at it, and the message given once nothing can fail any more; a failed start
  gives nothing. The cost: what is given goes to nobody (`MessageGiven`, issue 417), so the
  frontends find no message to mark the thread at - the web UI marks none, the terminal's
  `/thread list` tells no words it hangs at -, while its opening is announced in the line to
  everyone. Weighed and not taken: the last message of the thread the command was issued in -
  someone else's, and none in an empty thread -, and a `ThreadOrigin` that may also hang at a
  command recorded - a stored type, the forking of transcripts, the REST types and both frontends
  changed for one user. A command turn dropped after the sub-agent was started - the start runs
  as a procedure to its end - leaves the agent without the message; its next turn delegates
  nothing, so the delegation is let go and the thread closed with the sub-agent (issue 598).
- **For the agent's owners alone** (`owners_only`, as `/compact`): whoever issues it answers for
  the agent, as approving its calls - the command is the decision an approval of the tool leaves
  to them, and no other user has an agent of someone else start a sub-agent that its owner owns
  and that counts against the limits of its tree. A command turn asks for no approval;
  `requires_approval` stays the tool's, for what the agent starts on its own.
- **`/subagent @agent <personality> <brief>`**, named after the concept, as `/skill @agent <name>`
  is. A name may hold spaces, so the personality is the one of the component's list the arguments
  begin with, compared as names are, and the rest is the brief - a keyword such as `as` could
  stand in the brief itself; one not listed is refused with those that are, as the tool refuses
  it. Its answer is private, as `/participant add`'s: the channel announces the thread's opening
  to everyone anyway.

### Step 2: what the review of step 1 found

Two findings in the code around the command, planned by the maintainer as a step of their own:

- **`/skill` sets its message's time twice.** `SkillCommand::execute` writes `timestamp:
  Utc::now()` beside `..Message::new(…)`, which sets it already; `/subagent` builds the same kind
  of message without it. The field and the import of `chrono` go.
- **A plugin's command cannot answer a refusal as one.** `CommandError::Forbidden` carries the
  `Refused` that `Enforcement::require` hands over, which a plugin's error does not hold: what the
  access policy refuses a start by `/subagent` - `SubAgentError::Forbidden`, a text without the
  rule - reaches the issuer as "/subagent failed: …", while the tool answers it as a refusal
  (`ToolError::PermissionDenied`). Options: a variant of `CommandError` for a refusal told as a
  text, as `ToolError::PermissionDenied` is; or `Failed` kept, and documented as what a plugin's
  command answers. To be decided before it is implemented.

## Done when

- It is decided which command starts a sub-agent and to whom its answer goes, and it is
  implemented, with its permission checks in PERMISSION-CHECKS.md.

## History

- **2026-11-18** — Opened from issue 558's open points (*The command*).
- **2026-11-23** — Narrowed by the maintainer to a command of an agent (*Decision*): the command of
  the channel is not pursued, and a user working apart with an agent in a thread is moved to 595.
- **2026-11-25** — What stayed open decided by the maintainer (*The command*): `/subagent @agent
  <personality> <brief>`, for the agent's owners alone, its thread hung at the message it gives the
  agent.
- **2026-11-25** — Step 1 implemented: `SubagentCommand` of the component `subagents`, beside its
  tools (`CommandProvider<dyn AgentCommand>`), its text `started` in the prompt catalogue. Made one
  place each on the way: `Name::strip_from`, where a line begins with a name;
  `Situation::initiator_name`, in whose name `/skill` gives as well;
  `SubAgentError::is_correctable`, which the tool's errors and the command's are told apart by. Of
  several personalities the arguments begin with, the one taking most of them is taken. A start
  whose text cannot be given to the agent is ended again; no test holds that path, since the test
  world gives no catalogue that fails there. Accepted by the maintainer for now: the terminal
  completes the personality from all the user sees, not from the component's list. The findings of
  its review planned as step 2 (*Step 2*).
- **2026-11-25** — Decided by the maintainer: a variant of `CommandError` for a refusal told as a
  text. Step 2 implemented: `CommandError::PermissionDenied`, answered by REST as `403
  command_refused`; `/subagent` answers with it what the kernel refused and a depth not allowed, as
  the tool does; `/skill` no longer sets its message's time beside `Message::new`. On review: the
  variant reads "/… refused: …", so that a refusal is told from arguments refused; whether an error
  of sub-agents is to be corrected, refused or failed is one exhaustive match
  (`SubAgentError::class`, `ErrorClass`), which the tool and the command translate, in place of
  `is_correctable` and a second match beside it. Done.
