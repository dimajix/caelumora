# 558 — No agent can hand a task to a sub-agent and take back its result

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** An agent cannot hand a task to another agent that works on it apart, in a context of its own, and gives back only its result, nor can a user start such a sub-agent by command; the agent either does all of it itself, filling its transcript with every step, or asks another agent of the channel, where everyone hears every step.

## The problem

An agent that faces a task of many steps - searching through a workspace, reading a dozen pages,
trying something out - does all of it in its own turn. Every call and every result lands in its
transcript and stays in its context, although only the outcome matters for what follows; and two
such tasks cannot run side by side.

What exists comes close only in parts:

- **Another agent of the channel** can be asked by an `@`-mention
  ([CHANNELS.md](../docs/CHANNELS.md#chains-between-agents)). Then everyone in the channel hears
  the request, every answer and every turn the two agents take in reply, and each of them keeps all
  of it in its transcript.
- **An external agent** ([179](179-external-agents-via-a2a.md)) is given a task and gives back an
  answer, working apart from the channel - but only a remote agent reached by A2A, and only as a
  participant of the channel, which hears what it is given.

## What is wanted

The maintainer's idea, as stated:

- A sub-agent can be **started by a tool** and **by a command**.
- It works in a **channel of its own**, an additional channel of the current session.
- The **user is not an (active) member** of that channel.
- The model to orient by is the sub-agent of Claude Code, which the maintainer takes to be a kind
  of private conversation between the main agent and the sub-agent.

## How Claude Code does it

From its documentation ([sub-agents](https://code.claude.com/docs/en/sub-agents.md),
[commands](https://code.claude.com/docs/en/commands.md)) and from the description of its `Agent`
tool, as of 2026-11-11:

- **A sub-agent is started by a tool call** of the main agent, with a prompt - the brief - and a
  type, which brings its own tools, model and instructions (`.claude/agents/*.md`): the counterpart
  of a personality here.
- **It knows only the brief.** It does not see the main conversation; what it needs has to be in
  the brief.
- **Only its final report comes back**, as the result of the main agent's call. Its own calls and
  results stay out of the main agent's context. That is what it is for.
- **In the foreground or in the background.** In the background the main agent goes on, and is told
  when the sub-agent is done.
- **It can be continued**: the main agent sends it another message, and it goes on with its context
  as it was.
- **A fork** is a sub-agent that inherits the whole conversation so far instead of starting from
  the brief; only its result comes back, too. A fork cannot start further forks.
- **The user starts one by command**: `/subtask <task>` starts a fork in the background whose result
  returns to the conversation.
- **Code**: a sub-agent can be given a git worktree of its own, so that two of them do not change
  the same files.

So it is less a conversation than a **delegated piece of work**: one brief, work apart, one report
back. Rounds come about only when the main agent continues it, and every round is begun by the
main agent. The user writes nothing into it.

## What exists here

- **An agent's state in a channel** is its `AgentChannelState`, one per agent and channel: turn
  state, transcript, notes, private notes. The same agent in two channels has two of them, and two
  actors, each with its own turn.
- **Creating a channel** in a running session asks `UpdateSession`: only the session's owner may
  (`SessionLoop::create_channel`). A tool acts in its agent's owner's name
  ([SECURITY.md](../docs/SECURITY.md#in-whose-name-an-agent-acts)).
- **Who reads a channel**: it is owned by the session's owner, who may do everything with it, and
  read by the group of its participants ([SECURITY.md](../docs/SECURITY.md#ownership)). Whoever
  takes no part and does not own the session does not read it.
- **Approvals** go to the users taking part; a request nobody taking part may approve is denied at
  once ([APPROVALS.md](../docs/APPROVALS.md#approvals)). **Questions** of a call (`ask_user`) go to
  users of the channel; a handle that names none makes the call invalid.
- **Chains between agents** are counted from the last message of a user; past the moderator's
  `max_agent_hops` (default 3) nobody is asked any more.
- **An agent is made** of a personality by `AgentManager::create`; one of a channel's scope goes
  with the channel ([CHANNELS.md](../docs/CHANNELS.md#sessions)).
- **Long work of a tool**: a workspace command is followed for `wait_secs`, then handed back as
  running with its id, and followed or ended by further tools
  ([TOOLS.md](../docs/TOOLS.md#the-workspace)). A tool call is not waited for longer than its turn;
  dropped, it ends what it started.
- **Commands** are of the channel (`ChannelCommandProvider`) or of an agent
  (`AgentCommandProvider`, carried out by the agent itself in a turn of its own)
  ([COMMANDS.md](../docs/COMMANDS.md)).

## Considerations

### How the main agent is in the sub-channel

This is the question underneath; most others follow from it.

- **A. As a participant with an actor of its own.** The two agents talk as any two agents in a
  channel. But the main agent's actor there is a second one, with its own transcript and its own
  turns: what it learns there does not reach its turn in the main channel, except by `read_history`.
  The "private conversation" happens between the sub-agent and a copy of the main agent that the
  main agent does not hear.
- **B. Through the tool, without an actor there.** The tool puts the brief into the sub-channel in
  the main agent's name and takes the sub-agent's report back as its result, in the main agent's
  turn in the main channel. The sub-channel's history holds the brief, the sub-agent's work and its
  report; the main agent's own context only the call and its result - as in Claude Code. What is
  missing: someone in the sub-channel who speaks without an actor, or an internal actor of the
  sub-channel that speaks for the requester, as the approver is one
  ([CHANNELS.md](../docs/CHANNELS.md#building-a-channel)).

### In the foreground or in the background

- **The call waits** for the sub-agent's report. Simple; the main agent's turn is blocked
  meanwhile, and bounded by it: a cancel of the main turn drops the call, which has to end the
  sub-agent's turn.
- **The call hands back** at once, or after a while as a workspace command does, with an id; the
  report is fetched by a further tool, or arrives on its own as something the main agent hears and
  is woken by - as an external agent's task speaks on its own when it ends. Several sub-agents can
  work side by side.

Claude Code has both. The background asks for more: what wakes the main agent, and what happens to a
report that arrives while it is in a turn.

### What the user sees and may do

- **Seeing it.** The session's owner reads the sub-channel as its owner; any other user of the main
  channel does not, unless the sub-channel lets them. Whether the main channel shows the sub-agent's
  progress at the call that started it, and links to the sub-channel, is the web UI's to decide.
- **Approvals and questions.** With no user taking part, every call of the sub-agent that needs
  approval is denied, and `ask_user` cannot ask anyone. Either the sub-agent is given only tools
  that need neither, or the users of the main channel approve and answer for it - which approvals
  across channels do not do today -, or the user is a participant who does not write: what "not an
  (active) member" could mean - someone who approves, answers and watches, and is not counted as
  one who speaks.
- **Intervening.** Whether a user may cancel the sub-agent, write into the sub-channel or take it
  over.

### Which agent

- **A new agent of a personality**, made for the task in the sub-channel's scope and gone with it:
  the counterpart of Claude Code's types. [555](555-no-command-brings-an-agent-into-a-channel.md)
  makes agents of personalities by command as well.
- **An existing agent**, brought into the sub-channel - which raises whom it acts for there.
- **The main agent itself, as a fork**: it begins with its own state of the main channel. That
  needs copying its `AgentChannelState` into the sub-channel - the same step a branch modelled as a
  channel needs in [557](557-a-conversation-cannot-go-on-two-ways-at-once.md).

### Rules of the sub-channel

- **Chains.** Without a user, every exchange between the two agents is one chain, cut off after
  `max_agent_hops`; the sub-channel needs its own count, or it counts from the brief.
- **Who may start one.** Creating a channel is the session owner's alone; an agent acts with its
  owner's rights, so an agent of another user in a shared session could not start a sub-agent. It
  needs a rule of its own - as 557 does for branching.
- **What the sub-agent may.** It acts with its owner's rights - for a new agent, whoever made it,
  that is the main agent's owner. Whether it is bounded further by what the main agent may, and
  which tools it is offered, is to be decided.
- **Classification.** The sub-channel begins at the main channel's level; the report flows back into
  the main channel by the usual rules of what may be copied where.
- **The workspace.** The sub-channel shares the session's workspace; two sub-agents working side by
  side change the same files.
- **Lifetime.** Whether the sub-channel is kept for the record, ended or deleted when the report is
  back, and how many of them a session's list of channels can bear. A channel that names where it
  came from - the main channel and the call - would let the web UI group sub-channels under their
  origin, as 557 needs a lineage for branches.
- **Nesting.** Whether a sub-agent may start sub-agents itself, and how deep.

### The command

- **A command of the channel**, issued by the user: the user is the requester, and the report comes
  back into the main channel as a message - to whom, and in whose name?
- **A command of an agent** (`/subtask @bob <task>`), carried out by the agent in a turn of its own,
  as `/compact` is: the agent is the requester, as with the tool, and the report comes back to it.
  Closer to Claude Code's `/subtask`, which returns the result to the conversation it came from.

### The author's assessment

A, the main agent as an actor of the sub-channel, makes the conversation private in the wrong
place: the main agent in the main channel would not hear it. Claude Code's sub-agent is closer to
B: a call whose work is kept apart and whose report comes back as its result. B in the foreground
first, with a new agent of a personality and the command as an agent's command, keeps what is new
smallest: a sub-channel with a requester who speaks through a tool, and the approvals and questions
of the sub-agent going to someone. The background, forks and nesting can follow, each with its own
cost. Not decided.

## Open questions

- How is the main agent in the sub-channel (A or B), and who speaks the brief there?
- What does "not an active member" mean for the user: reading only, or approving and answering too?
- Foreground, background, or both?
- A new agent, an existing one, or a fork of the main agent?
- Who may start a sub-agent, and is it bounded by what the main agent may?
- Is the sub-channel kept, and how is it shown?

## Done when

- The model is decided - how the main agent is in the sub-channel, what the user is there, and what
  comes back how - with the reasoning recorded here.
- The steps are planned in this issue, or in issues of their own.

## History

- **2026-11-11** — Opened from the maintainer's idea: sub-agents started by tool and command, in a
  channel of their own in the session, without the user as an active member, oriented by Claude
  Code. Related to [557](557-a-conversation-cannot-go-on-two-ways-at-once.md): a fork needs the same
  copy of an agent's state, and both need a channel that names where it came from.
