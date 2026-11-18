# 558 — No agent can hand a task to a sub-agent and take back its result

**Status:** done
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

### As a thread of 557

[557](557-a-conversation-cannot-go-on-two-ways-at-once.md#threads-a-third-level-below-the-channel)
now considers threads: a third level below session and channel, a conversation with its origin in a
channel - the message it hangs at and the channel as it stood when it was opened -, kept apart from
the channel, with a way back by a result. A sub-agent would be such a thread: opened by an agent
through a tool or a command, with few taking part, begun with a brief - or, as a fork, with the
conversation -, and its result coming back as the result of the call that opened it.

That would answer some questions here together with 557, not decided:

- **Where it lives.** The sub-channel is a thread of the main channel, not a channel of the
  session: the session's list of channels does not fill up, and the web UI shows it at the call
  that opened it.
- **What it takes along.** Its owner and its classification at the start come from the main
  channel, as for every thread.
- **Seeing it.** Whoever reads the main channel could read its threads, the sub-agent's among them;
  approvals and questions of the sub-agent could go to the users of the main channel, which would
  give "not an (active) member" a meaning: a user who reads and answers there, but does not speak.
- **Who may start one** follows who may open a thread.

It leaves open how the main agent is in the thread (A or B above), and whether an agent that does
not take part in the main channel may take part in its thread - which a new agent of a personality
would.

Since 557's decision of the same day, a thread is a part of its channel and shares its participants
and notes. Two consequences for a sub-agent:

- **The sub-agent takes part in the main channel** - an agent of the channel's scope, made for the
  task -, or threads get participants beyond their channel's, which 557's decision does not provide
  for. The users of the main channel are then participants of the thread by construction, and may
  read it, approve and answer there.
- **The main agent is in the thread with its actor of the channel**, its transcript there a branch
  of its transcript in the channel: what it learns in the thread reaches its own line by the
  result, or by the notes they share. That is 558's B in substance - only the result comes back -,
  with the main agent speaking the brief itself instead of a tool speaking for it.
- **A closed circle** (557's *Decision*) would keep who speaks in a sub-agent's thread to the main
  agent, the sub-agent and the users who approve and answer for it; everyone in the channel still
  reads it, and the sub-agent still takes part in the channel.

## Open questions

- How is the main agent in the sub-channel (A or B), and who speaks the brief there?
- What does "not an active member" mean for the user: reading only, or approving and answering too?
- Foreground, background, or both?
- A new agent, an existing one, or a fork of the main agent?
- Who may start a sub-agent, and is it bounded by what the main agent may?
- Is the sub-channel kept, and how is it shown?
- Is the sub-channel a thread of 557 (*As a thread of 557*)?

## Decision

The maintainer's decision of 2026-11-17, on the author's proposal. It goes beyond *As a thread of
557* in one point: the main agent does not take part in the sub-agent's thread, so what is said
there of the main agent in the thread with its actor of the channel no longer holds.

- **A sub-agent works in a fresh thread** of the channel
  ([557](557-a-conversation-cannot-go-on-two-ways-at-once.md#decided-on-2026-11-17)): it shares
  the channel's notes, artifacts and properties, but its conversation begins with the brief.
- **Starting it does not block.** The tool opens the thread, writes the brief into it as its first
  message, in the main agent's name, and returns at once with the thread's id. The main agent's
  turn ends as any turn does. So a sub-agent always works in the background, and several can work
  side by side.
- **The main agent does not take part in the thread.** It neither hears it nor is asked there; its
  work stays in the thread it started the sub-agent from.
- **The way back is given to read.** The sub-agent's answer to the brief - the message of the turn
  that answers it, which the events of a turn name (`correlation_id`) - is given to the main agent
  to read in the thread it started the sub-agent from, as a `Given` entry with the sub-agent as its
  giver - "@researcher gave you this to read:", as a skill given with `/skill` -, together with a
  request to respond. It is not a message of that thread, and no other agent hears it; the users
  see the answer in the sub-agent's thread and at the call that started it. So the main agent sees
  the result as Claude Code shows it, as something that came back to the call, and answers in its
  own thread with what it makes of it. A tool by which the sub-agent reports its result explicitly
  stays the alternative, should the answer to the brief not suffice to tell the result.
- **Asking again** is a second tool: it writes into the thread in the main agent's name, and the
  answer to it is given to read in the same way.
- **The thread's policy**: the sub-agent answers what the main agent writes there, and mentions.
  What it answers a user who writes in the thread is no result and is given to nobody.
- **No messages across threads in general.** Directed messages from one thread to an agent in
  another, kept out of that thread's history, were considered and are not pursued: a second way of
  speaking beside the channel, with a visibility and a record of its own, while what is given to
  read covers the one case that needs it.
- **The sub-agent is deleted when its work is ended**, and with it its thread is closed for good
  and cannot be opened again. Its work is ended by the main agent, through a tool, or by a user
  closing the thread; deleting it after its first result would leave nothing to ask again.
- **Which sub-agents may be started** is the tool's configuration: a list of the personalities it
  may start an agent of, whose components bring its tools and everything else. The sub-agent is a
  new agent of one of them, owned by the main agent's owner, who has to be able to read the
  personality, as for any agent made.

What this answers of the open questions above: the main agent is in the thread neither as A nor as
B - the tool speaks the brief, and the answer comes back given to read -; a sub-agent always works
in the background; it is a new agent of a listed personality; and its thread is shown at the call
that started it and ends with the sub-agent.

- **The sub-agent takes part in its own thread alone**, decided as well. Every participant of a
  thread is a participant of the channel; but a sub-agent that took part in the channel's line
  would hear it, and could be chosen there by the channel's policy. So it is a participant of the
  channel - its rights, the channel's notes and properties - that takes part in its thread and in
  no other, the channel's line included. That departs from 557's proposal that all of the
  channel's participants take part in its line.
- **Its notes and 581.** What is noted in a channel is read from the agents taking part in it
  ([581](581-the-notes-of-an-agent-that-left-fall-off-the-board.md)), so a sub-agent's notes may
  vanish when it is deleted, and only its result and its artifacts remain. 581 is no precondition:
  if they vanish, that is one more finding for 581, not a reason to wait for it.

### What is still open

- **How the sub-agent is kept to its thread**: by the access lists, or by another mechanism.
- **An idle sub-agent.** Whether one the main agent never ends is ended after a while.
- **Whether users may write in its thread**, or the thread is a closed circle of the sub-agent
  alone.
- **The command.** A user starting a sub-agent by command, as wanted: to whom its answer is given,
  and in which thread.
- **Bounding its rights** by what the main agent may, beyond the list of personalities.

## Plan: the crate caelumora-subagents

The author's proposal of 2026-11-17 - not decided but where it says so. The channel's building
blocks it uses are planned in
[557](557-a-conversation-cannot-go-on-two-ways-at-once.md#plan-step-2-threads-as-building-blocks).

### Where the code contradicts the decision

Traced on 2026-11-17:

- **"Writes the brief into it as its first message, in the main agent's name."** Only an actor
  speaks in its own name (`OriginEventSender`). Said through the main agent's actor, the brief is a
  message of its turn - in its transcript, with its `turn_id` -, and asks the sub-agent at best at
  that turn's end, in the line. Nobody else can say it in the main agent's name.
- **"The sub-agent's answer to the brief - the message of the turn that answers it, which the
  events of a turn name (`correlation_id`)."** A turn answers every request taken in together, in
  the context of the last; a request heard during a turn, or right after a turn that answered it,
  is dropped as answered; a retry answers in its own context; and an agent asleep, a quiet
  channel or a chain cut off makes no turn at all.
- **"As a skill given with `/skill`."** That is an agent's command: admitted only from a user,
  admitted again with its sender's rights as it is delivered again from the history - a deleted
  sub-agent's none -, shown to users, and asking nobody to respond.
- **"Deleted when its work is ended, and its thread is closed for good."** A session lets nobody
  go (issue 564), so every sub-agent stays among its participants; a deleted agent rests in its
  channels and comes back with a restore; and while a sub-agent of an agent whose owner does not
  own the session lives, its channel's scope keeps the channel from being deleted
  (`ScopeNotEmpty`).
- **"And mentions."** A mention of an agent that takes no part in the thread asks nobody there by
  the mention: the thread's policy chooses instead.

### Decided

The maintainer, on 2026-11-17: the channel offers building blocks alone (557), and the sub-agents'
work - making one, carrying its brief in and its answer out, ending it - lies in a crate of its
own, `caelumora-subagents`.

### The crate

Built as `caelumora-skills`, `caelumora-memory` and `caelumora-artifacts` are (PLUGINS.md, *What
a plugin is*), a plugin and a library at once, cut by features:

| Feature | What | Trusted |
|---|---|---|
| `api` | the writer `SubAgentWriter` and its errors | what the agent side is given of the service |
| `kernel` | `SubagentsPlugin`: binds the writer (`extend_writers`), whose work is made of the kernel's building blocks | as the kernel |
| `library` | `SubagentsLibrary`: the tool, offered where the writer is (`Situation::has_writer`), configured with the personalities it may start an agent of | for nothing |

```rust
// caelumora-subagents, feature api
/// What an agent's work does with sub-agents: starts one on a brief, asks it again, ends it.
pub trait SubAgentWriter: Send + Sync {
    /// Starts an agent of `personality` on `brief`, in a thread of its own at the call `origin`:
    /// returns the thread at once.
    async fn start(&self, personality: &str, origin: ThreadOrigin, brief: String)
        -> Result<ThreadId, SubAgentError>;
    /// Gives the sub-agent of `thread` `text` to read and asks it to respond, once it answered.
    async fn ask(&self, thread: ThreadId, text: String) -> Result<(), SubAgentError>;
    /// Ends the sub-agent of `thread`, and closes the thread.
    async fn end(&self, thread: ThreadId) -> Result<(), SubAgentError>;
}
```

### How it works

- **Starting.** The writer makes an agent of the personality (`AgentManager::create`, in the
  channel's scope, owned by the main agent's owner, who has to be able to read the personality),
  founds a thread at the call with the new agent taking part in it alone (557's `open_thread`), and
  delegates the brief to it (557's `delegate`), on behalf of the main agent. The call returns the
  thread's id at once; the main agent's turn ends as any turn does.
- **The kernel's delegate carries the rest** (557, *Delegating*): it gives the sub-agent the brief
  to read, naming the main agent as the giver, and asks it to respond - by a request of its own, so
  that no policy, mention or chain limit decides it -; it takes the sub-agent's next turn end in
  its thread as the answer - sound because the thread is a closed circle in which the delegate
  alone gives the sub-agent anything, one brief at a time -; and it gives the main agent, in the
  origin thread, what came of it, and asks it to respond: the answer, that the turn ended without
  one, that it was given up and why, or that the sub-agent was not asked. The main agent so never
  waits for nothing; in a turn, it takes it in once its turn ended. The crate knows nothing of
  this but that it delegates.
- **Asking again** is `ask`: the writer delegates the text once more, which the delegate gives the
  sub-agent once the open brief is answered.
- **Ending** is `end`: the sub-agent is let go from its channel and its session, is deleted (E3
  below), and the thread is closed with its delegate.
- **What users see.** The thread is read by everyone in the channel; that a brief was given and
  what was given back are shown there and at the call that started it (557, T5).

### Decided on the open points

The maintainer, on 2026-11-17:

- **E3 - How a sub-agent ends.** It is let go from its channel and from its session - out of both
  lists, kept as a former participant of the channel - and deleted, never purged: what it did, its
  document and its state are kept, for tracing what happened. Restored, it comes back into nothing.
  That a session lets nobody go turned out to be a gap, not a decision: letting go from a session
  is issue [590](590-a-session-lets-nobody-go.md), which ending needs first.
- **The answer wakes a main agent that sleeps**, as a mention does.
- **Cancelling, a thread or the whole channel.** A cancel ends the agents' work in a scope: one
  thread, or the channel. Being quiet becomes the state of a thread, held by its `Thread` (557,
  D8): a cancel of a thread makes it quiet, a cancel of the channel every thread, as it does today;
  waking goes the same two ways, both under the permissions of today (`CancelChannel`,
  `WakeChannel`). So a user who cancels the sub-agent's thread ends its turn, the delegate gives the
  main agent that it ended without an answer, and the main agent, whose thread is not quiet, is
  asked. A cancel of the channel - a user wanting everything to stop - still lets the delegate
  give, but nobody is asked until the channel is woken. A brief given into a quiet thread (`ask`)
  is refused - the thread is quiet - instead of waking it, so that the main agent cannot undo a
  user's cancel.
- **Names**: an adjective and the personality (`curious-researcher`, `steady-reviewer`), drawn by
  the writer, never one the channel has had - among its participants or its former ones -, so that
  its history never shows two sub-agents under one name. No names of real people, as Docker's are.
- **An external agent as a sub-agent** is issue
  [591](591-an-external-agent-cannot-be-a-sub-agent.md): the tool starts agents of internal
  personalities alone, and refuses an external one with an error.

Still open from above: the command by which a user starts a sub-agent, and bounding its rights by
what the main agent may.

## Done when

- The model is decided - how the main agent is in the sub-channel, what the user is there, and what
  comes back how - with the reasoning recorded here.
- The steps are planned in this issue, or in issues of their own.

## History

- **2026-11-11** — Opened from the maintainer's idea: sub-agents started by tool and command, in a
  channel of their own in the session, without the user as an active member, oriented by Claude
  Code. Related to [557](557-a-conversation-cannot-go-on-two-ways-at-once.md): a fork needs the same
  copy of an agent's state, and both need a channel that names where it came from.
- **2026-11-16** — *As a thread of 557* added: 557 now considers threads, and a sub-agent would be
  one, opened by an agent, with its result as the result of the call. Not decided. On the same day
  557 decided that a thread is part of its channel; its consequences for a sub-agent added there.
- **2026-11-17** — Decided by the maintainer (*Decision*): a sub-agent works in a fresh thread, is
  started without blocking, the main agent takes no part in its thread, its answer is given to the
  main agent to read in the thread it was started from, and it is deleted when its work is ended;
  the tool lists the personalities it may start; it takes part in its own thread alone; 581 is no
  precondition. What is still open is listed there.
- **2026-11-17** — The decision traced through the code (*Where the code contradicts the
  decision*); the maintainer decided that the channel offers building blocks alone and the
  sub-agents' work lies in a crate of its own, `caelumora-subagents`. Proposed (*Plan: the crate
  caelumora-subagents*): the crate's three features, the writer, and a delegate - an internal actor
  of the crate's kind - that carries the brief in and the answer out by giving to read.
- **2026-11-17** — Decided by the maintainer: the delegate is a concept of the kernel - delegating,
  a building block of the channel (557) -; the crate makes an agent, founds its thread, delegates
  to it and ends it, and registers no kind of actor.
- **2026-11-17** — Added to *To decide*: whether an external agent may be a sub-agent, with what it
  would need.
- **2026-11-17** — The points of *To decide* decided by the maintainer (*Decided on the open
  points*): a sub-agent whose work is done is let go from its channel and its session and deleted,
  never purged - letting go from a session opened as issue 590 -; its answer wakes a main agent
  that sleeps; a cancel ends the work of one thread or of the whole channel, being quiet a thread's
  state, and a brief into a quiet thread is refused; a sub-agent is named by an adjective and its
  personality, never by a name its channel has had; external agents as sub-agents opened as issue
  591.
- **2026-11-17** — The steps of 557's 2b decided, with this crate as step 5 and the frontends as
  step 6 ([557](557-a-conversation-cannot-go-on-two-ways-at-once.md#the-steps-of-2b)).
- **2026-11-18** — The crate `caelumora-subagents` (557's step 5b of 2b): the writer
  `SubAgentWriter` - `start`, `ask`, `end` -, bound by `SubagentsPlugin` to the session of every
  piece of work of an agent that runs, made of the kernel's building blocks alone: the agent made by
  the work's owner in the channel's scope, of an internal personality alone (591), named by an
  adjective and the personality, never by a name the channel has had; the thread founded with it at
  the call; the brief delegated to it; and ending it, by the agent that started it alone, as letting
  it go from the session, closing its thread and deleting the agent. The component `subagents`
  (`SubagentsLibrary`), configured with the personalities its tools may start an agent of, and its
  tools `start_subagent`, `ask_subagent`, `end_subagent`. Still open: the command by which a user
  starts a sub-agent, an idle sub-agent, bounding its rights by the main agent's, and a user
  closing its thread.
- **2026-11-18** — Decided by the maintainer on 5b's review, as a step of its own: the personality
  found as the kernel finds it from the channel's place (`Views::personality_named`, public); a
  refusal told from a failure (`SubAgentError::Forbidden`, answered as the tool's
  `PermissionDenied`; `Failed`, worded as users are told); `start_subagent` asking for approval as
  its component is configured (`requires_approval`, not by default); and an open brief dropped as
  its sub-agent is ended, said in TOOLS.md.
- **2026-11-18** — These decisions implemented: `Views::personality_named` public, the writer
  finding the personality from the channel's scope; `SubAgentError::Forbidden` and `Failed` in place
  of `Refused`, worded by `caelumora_kernel::error::user_message`, the chain of a failure logged;
  the component's `requires_approval` (`never` unless configured); TOOLS.md saying what becomes of
  an open brief as its sub-agent is ended. On review: a refusal worded by what was refused
  (`Refused`), the writer's own conditions in words of their own; a work that may not reach its own
  channel answered as not found, as `channel_for` answers it, and said so.
- **2026-11-18** — The open points after the crate, opened as issues of their own: the command
  (596), bounding a sub-agent's rights, their number and their depth (597), a sub-agent nobody
  ends (598), and writing into its thread with the view of a thread in the frontends (595). With
  them the steps are planned, as *Done when* asks; what is built is the crate and the building
  blocks of 557's 2b.
