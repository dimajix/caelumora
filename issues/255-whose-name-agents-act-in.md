# 255 — Agents read in their owner's name and act in the name of whoever asked them

**Status:** open
**Area:** Identity & access management

## The problem

An agent acts in two security contexts, and which one an action is asked in depends on where in the
code it happens, not on a rule anyone decided (see
[SECURITY.md](../docs/SECURITY.md#in-whose-name-an-agent-acts)):

*   **Its own context** - its owner as initiator, the agent as caller (`Agent::security_context`,
    issue [87](87-security-context-in-entities.md)). Its views are made in it, so everything it
    reads through them is asked in its owner's name: other channels' history and notes, its private
    notes, its memory's recall, and the session's workspace through its channel. So are its
    components built, and what it says when nobody asked it.
*   **The turn's context** - the request's initiator, whoever asked the agent, continued as the
    agent (`Answering::to`). Its answers are published in it, its tool calls admitted in it
    (`CallTool`) and their results admitted into the channel, the credentials of a call read in it
    (`ReadSecret` for an MCP server or an external agent), and the questions of a call put in it.

Both meet in a single call. When bob asks alice's agent to run a command (issue
[251](251-open-terminal.md)), the call is asked as "bob as helper" and the workspace it runs in is
reached as "alice as helper" - the security suite pins exactly that
(`an_agents_command_is_called_for_who_asked_and_reaches_the_workspace_through_its_channel`).
Each half has a reason: an agent must not know more than the person responsible for it (issue
[142](142-roles.md)) and passes on what it read (issue
[131](131-agent-memory-bypasses-policy.md)); a person who asks lends the agent their authority for
the turn. But nobody decided that the two halves belong together this way, and where a new
component lands - in a view, in the turn - silently decides whose rights it uses.

## Why it matters

- **A confused deputy in one direction.** What the agent reads, it reads with its owner's
  clearance and brings into a channel where someone else asked. The flow rules keep what is read
  within the channel's classification, so a reader of the channel learns nothing above it - but
  within that bound the asker reaches, through the agent, whatever the owner may read and they may
  not: another channel of the session they are not in, the owner's workspace.
- **A borrowed credential in the other.** A call reads its credential in the asker's name, so an
  MCP server's token must be readable by every person who may ask the agent - or the call fails for
  some of them and not for others, depending on who asked, not on what the agent is.
- **Inconsistency is a risk of its own.** A tool that reads through a view and acts through the turn
  is checked by two people's rights at once, and its author has to know which part is which.
  External agents, notes, memory and the workspace each made that choice on their own.
- **Delegation chains.** An agent asked by an agent asked by a person: the turn's context continues
  the first initiator, the views the owner of the last agent. Issue
  [12](12-user-and-agent-permissions.md) asks for a rule over the whole chain; there is none yet.

## Considerations

- **Keep the split, decide it.** Reading in the owner's name, acting in the asker's, written down as
  the rule - with every kind of action assigned to one side explicitly (in `ActionType`, say), so
  that it no longer follows from where the code sits. Cheapest; leaves the confused deputy within
  the channel's bound.
- **Everything in the owner's name.** The asker only triggers the agent; the agent is its owner's
  delegate in all it does. One subject per agent, credentials of its own (12's *grant*). Whoever may
  ask the agent uses its owner's rights - acceptable only where asking is itself restricted, which
  it is not today: every member of a channel may address every agent in it.
- **Everything in the asker's name.** The agent can do and read only what the asker could. No
  deputy at all, but an agent with a credential its users lack is impossible, and what an agent
  remembers or noted in one turn would be readable or not depending on who asks next.
- **The intersection.** Every action asked in both names (12's conservative rule): the agent reads
  and does only what its owner and the asker may. Safe and consistent; it costs a second question
  per check, forbids the legitimate case of an agent holding a capability its users lack, and needs
  a rule for the chain.
- **Who may ask.** Part of the answer may lie not in the checks but in who may ask an agent to act
  at all - an `AskAgent` action decided by the owner, say - which would make "the owner's name"
  safe where today it is not.

Related: [12](12-user-and-agent-permissions.md) (the general rule for combining rights, which this
is a concrete case of), [142](142-roles.md) (an agent's clearance is its owner's),
[181](181-where-roles-decide.md) (where roles enter a check),
[213](213-external-task-taken-up-keeps-old-work.md) (whose authority an external task keeps).

## Done when

- It is decided, per kind of action, in whose name an agent asks - and the decision is in
  SECURITY.md and in the code at one place, not spread over views and turns.
- The security suite pins the decision for reading another channel, the workspace, a tool call, a
  credential and a delegation through a second agent.

## History

- **2026-10-08** — Opened at the maintainer's request, after the security suite test of step 5 of
  [251](251-open-terminal.md) showed a command called in the asker's name and its workspace reached
  in the owner's. The current model was written down in SECURITY.md (*In whose name an agent acts*)
  as it is, not as decided.
