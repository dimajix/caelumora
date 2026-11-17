# 557 — A conversation cannot go on in two directions at once

**Status:** in progress
**Type:** question
**Priority:** medium
**Area:** Sessions and channels
**Summary:** A channel's conversation has one line, so whoever wants to follow a second direction from where it stands - with the same participants and what they know so far - can only say it in the same line, where both directions mix, or start a channel that knows nothing of what was said.

## The problem

A conversation in a channel goes on in one line. When it reaches a point from which two directions
are worth following - two approaches to try, a side question that should not derail the main one -
there are two ways today, and neither is good:

- **Both in the same line.** The directions mix: every agent's transcript holds both, every answer
  is given with the other direction in its context, and which message belongs to which is left to
  the reader.
- **A new channel.** It knows nothing of what was said. Its agents can read the other channel with
  `read_history` ([MEMORY.md](../docs/MEMORY.md#reading-other-channels)), but what they read is
  material, not a conversation they took part in: their own tool calls, results and reasoning are
  not there.

## What is wanted

The maintainer's idea, as stated:

- A **branch can only be made at the current point** of the conversation, never by replacing an
  older message. A branch is a second continuation of the conversation as it stands now.
- **Every participant may answer on any branch.** Branches live side by side; nobody switches the
  channel from one to the other.
- A **consideration, explicitly neither a recommendation nor a decision**: whether a branch should
  simply be modelled as a new channel. That would probably be simpler technically; whether the
  result meets what one expects of a branch is unclear.

## How this differs from 08

Branching was planned before, in [08](08-retry-and-edit-past-answers.md), three times over, and
rejected on 2026-10-05 in favour of [234](234-modify-revoke-and-retry-messages.md). Everything in
that plan rests on one assumption: a channel and every agent in it have **one active path**, and a
switch moves everyone onto another. What made the plan hard came from that switching - stores that
hold different messages in different order, answers that arrive for a branch no longer shown, an
external agent that cannot take back what it was given.

The idea here gives that assumption up from the other side:

- **Nothing is replaced and nothing is switched for everyone.** Branches only grow. What 08 needed
  for replacing - `Message::after` as the fork point of a turn, the rule of what is replaced, the
  late answer of a branch no longer active - is not needed.
- **Several branches are live at once.** That is new, and it is where the cost moves to (below).
- **Correcting stays 234's.** Modifying or revoking a message and asking an agent to answer again
  are done where the message stands, without a fork; this issue changes nothing of that. Branching
  is for going on in two directions, not for undoing.

Of 08, step 1 stays useful: every chain (`Chain<E>`) is a tree with a head. Today its branches are
an agent's given-up turns ([145](145-ended-turns-leave-a-record.md)), the calls of an external agent
([205](205-external-agent-message-context.md)) and what a compaction began anew
([AGENT-TURN.md](../docs/AGENT-TURN.md#the-transcript-as-a-chain)) - all with one head, and an agent
holds only its active branch in memory ([380](380-transcript-window.md)).

[19](19-memory-tiers.md) lists 08's steps as its phase 3, *branching*; this issue takes that place
once it is decided.

## What exists

- **What an agent keeps of a channel** is its `AgentChannelState`, one per agent and channel: its
  turn state (`processing_state`), its transcript, its notes, its private notes, and what its prompt
  and offered tools were. Everything an agent would have to keep *per branch* is in it.
- **A channel** is created in a running session (`SessionLoop::create_channel`); the session's
  change asks `UpdateSession`, so only the owner may create one. A channel's name is unique in its
  session. A `Channel` names no channel it came from.
- **A newcomer's share of the history** (`HistoryAccess`: all, none, the last n messages or days)
  narrows what a participant added to a channel sees of *that* channel's history. Nothing gives a
  channel the history of another.
- **Reading other channels**: `read_history` and `read_notes` reach another channel of the session
  under the rules of [MEMORY.md](../docs/MEMORY.md#reading-other-channels) and
  [CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md#reading-another-channel).
- **What is new to a user** is kept per user and channel (`UserChannelState`).

## Considerations

### The question underneath

Is a branch an **alternative within one conversation**, or a **separate conversation that shares a
beginning** with another? The two sentences of the idea pull in different directions here:

- What chat systems have taught people to expect of a branch - `< 2/3 >` at a message, one
  alternative shown, the others a click away - is the first: alternatives, one of them current.
  That is 08's model.
- *Every participant may answer on any branch* is the second: several conversations live at the
  same time, each with its own participants answering, each with something new in it while another
  is shown.

Which of the two is meant decides more than the technique; it is the question to settle first.

### A. Branches within one channel

The channel keeps one history, a tree with several live heads.

- **Every chain gets several heads**: the channel's history and every agent's transcript, notes and
  private notes. Whatever reads "the conversation" has to name the branch it reads.
- **Every message and event names its branch**: what a user sends, what the moderator routes, what
  an agent answers, every change of 234, the REST API and the streams.
- **An agent keeps its state per branch**: transcript head, notes, private notes, turn state, what a
  retry may take back - that is, nearly all of `AgentChannelState` once per branch, inside the one
  channel. An agent asked on two branches answers one after the other or runs two turns at once.
- **The moderator routes per branch**, mentions included.
- **Compaction and the transcript window** ([380](380-transcript-window.md)) work per branch; an
  agent holds several active branches in memory.
- **Participants are the same on every branch** by construction.
- **The web UI** shows a tree: which branch is shown, what is new on the others, where they fork.

### B. A branch is a new channel

Branching creates a channel in the same session that begins with the conversation of its origin up
to the fork point and goes on alone from there.

What it has without anything new: its own loop, moderator, routing and turn per agent; its own
`AgentChannelState` per agent; its own permissions, classification and unread count; the REST API,
the streams and the web UI as they are. Agents on two branches work independently, as they do in
two channels today.

What it needs:

- **A lineage.** A channel would name what it branched from: the channel and the position in its
  history. Today nothing does.
- **The beginning of the conversation.** Either copied into the new channel's history, or read
  through the reference from the origin's history up to the fork point. A copy is independent of
  its origin; a reference keeps one record of what both share.
- **The agents' states at the fork.** Either each agent's `AgentChannelState` is copied (its
  transcript with its internal entries and the state of a compaction, its notes, its private
  notes), so that it goes on as if nothing had happened; or the agents are given the beginning as a
  newcomer is, and lose their tool calls, results and reasoning of it. The first is the point of a
  branch; it is also the largest part of the work.
- **Who may branch.** Creating a channel is the session owner's alone today. If every participant
  may branch, branching needs a rule of its own, as cancelling and waking have one.
- **A name**, since names are unique in a session: one generated from the origin's, or asked for.
- **Changes to the shared beginning** (234): a message of the beginning modified, revoked or
  reclassified - in a copy, the change stays in the channel it was made in; with a reference, it
  has to reach every channel that shares the message, with every agent that holds it.
- **Classification.** The beginning flows from the origin into the branch; at the same level that
  is allowed, and afterwards each is raised or lowered on its own.
- **Deleting the origin**: harmless for a copy, a question for a reference.
- **External agents**: a remote agent has one context; going on in a branch means a new remote
  context that is given the beginning again, or the external agent does not come along.

What falls out of it, wanted or not:

- **Participants can differ** between branches: someone is added to one branch only.
- **Branches can read each other** with `read_history` and `read_notes`, under the usual rules.
- **Many channels.** A session that branches often fills its list of channels.

### C. B in the model, A in the presentation

Branches are channels as in B, but the web UI shows a channel with an origin as a branch of it: a
mark at the message it forked from, a way to go from one to the other, and the branches grouped
under their origin rather than beside every other channel. This answers the question of
expectation in the frontend, at least partly, without changing the model; it does not answer the
question underneath - an alternative shown instead of the other, or a conversation beside it.

### For all of them

- **The world does not branch** ([20](20-transactional-effects.md)): what a tool did on one branch
  is done on all; two branches working on the session's workspace work on the same files - now at
  the same time, unlike in 08.
- **A turn under way at the fork point**: branching only while nobody answers, or its answer lands
  on the branch it began on alone.
- **Agent memory** across channels and sessions does not branch
  ([MEMORY.md](../docs/MEMORY.md#common-rules)); notes branch with the conversation in every option.

### How Claude Code does it

For comparison, as its documentation describes it on 2026-11-11
([checkpointing](https://code.claude.com/docs/en/checkpointing.md),
[sessions](https://code.claude.com/docs/en/sessions.md),
[sub-agents](https://code.claude.com/docs/en/sub-agents.md)):

- **Rewinding** (`/rewind`) goes back to an earlier prompt: the conversation, the files Claude
  edited since, or both. What was rewound is not kept as an alternative to return to - the
  counterpart of 234, not of 08.
- **`/branch`**, and `--fork-session` on resuming, copy the conversation up to that point into a
  new session with an id of its own and switch into it; the original stays unchanged and can be
  resumed. **`/fork`** copies it into a new session that runs in the background while one goes on
  working in the original; nothing that one of them does reaches the other.
- **A forked subagent** inherits the whole conversation, works on one task and returns only its
  result to the conversation it came from: a branch that is merged back, by its result.
- **No session has two live branches.** Branches side by side are sessions side by side, with a
  copied beginning and no connection afterwards.

That is B with a copy, in a system with one agent per conversation, whose whole state is its
transcript: a copy of the transcript is a copy of everything. Here the same needs a copy of every
agent's `AgentChannelState`. The world does not branch there either: for code, `/fork` has the
copy make a git worktree of its own before it changes anything. For a session's workspace here
nothing comparable exists.

### The author's assessment

Not a decision, and not the maintainer's: the maintainer named B as a consideration only.

What A has to keep per branch is what a channel already keeps per agent - a transcript, notes, a
turn, a moderator's routing. A would build a second kind of conversation inside the channel, next
to the channel, and every reader of a chain would have to learn which branch it reads; that is one
more concept and a special case in every place that reads. B keeps one concept, a conversation, and
adds a relation between conversations; its real costs are copying the agents' states at the fork
and the rule for changes to the shared beginning. So, if the answer to the question underneath is
*conversations beside each other*, B with the presentation of C looks like the simpler design; if
it is *alternatives, one shown*, the idea does not need *every participant on any branch*, and 08's
plan is the closer starting point, with the reasons it was rejected still standing.

### Threads: a third level below the channel

An assessment from the user's side, of 2026-11-16 - not a decision.

**Conversations side by side.** Alternatives with one shown (`< 2/3 >`) are a pattern of chats with
one person in them. With several people and agents in a channel, someone would have to decide which
alternative is current for all of them - the switching 08 failed on. Group chats - Slack, Discord,
Teams - have answered the same need alike, with threads. So the answer to the question underneath
would be *conversations side by side*.

**Where a thread sits.** Unlike in those systems, two levels exist here already: a session is a
large working group with a common goal, a channel a small circle specialised on a part of it. A
thread would be a third level below them: a passing strand of one circle's work, not a circle of
its own. It belongs to its channel, not to the session: it is not in the session's list of
channels, and it takes its owner, its participants and its classification at the start from its
channel. A strand that grows into a lasting circle is a new channel.

**What the user sees and does.**

- **A thread hangs at a message and knows everything up to now.** The message it hangs at says what
  it is about; what its agents know is the channel as it stands when the thread is opened. So a
  thread can be opened at an older message without anything being replaced: the rule of branching
  only at the current point holds for the context, while opening one feels free to the user.
- **In the channel it stays small**: a mark at its message - "thread · 4 answers · @alice,
  @coder" - that opens it beside the channel, and an unread count of its own.
- **The channel's agents do not hear it.** Their contexts stay free of it; that is what it is for.
- **A result can flow back.** At any time, or at its end, a thread hands a result to its channel as
  a message: a summary an agent writes, or a message picked from the thread. The channel's agents
  hear only that. This answers the question of merging back: by a result, never by the history.
  Two approaches are tried in two threads, and the better one's result is taken over.
- **Whoever may write in the channel may open a thread**: it is an act in the channel, not the
  creation of a channel in the session, which stays the owner's.
- **The agents come along as they are.** Opening takes a snapshot of every agent's
  `AgentChannelState`. Without it, an agent in the thread has forgotten what it read and did itself,
  and the thread feels like a new chat. A copy also behaves predictably: that a correction made in
  the channel after the thread was opened does not reach the thread is what one expects of a thread
  anyway. Of B's options, this is the copy, for the beginning and for the agents' states alike.

**One concept for 557 and 558.** A sub-agent of [558](558-no-agent-hands-a-task-to-a-sub-agent.md)
is a thread with three differences: an agent opens it, not a person; few take part in it; and it
begins with a brief instead of the conversation - or, as a fork, with the conversation after all.
Its result flows back as the result of the call that opened it. 558 already notes that both need a
lineage and the copy of an agent's state; the way a result flows back is common to both as well.
One concept - a conversation with its origin in a channel and a way back into it - would serve both
issues.

**What is not done.** A, branches within one channel, would be a second kind of conversation next
to the channel. Alternatives with one shown stay at the level of a single answer, which is
[238](238-retry-of-an-older-answer.md)'s.

**What has to stay visible.** The world does not branch: two threads work on the same workspace,
and the web UI should say so; versions of a workspace ([256](256-workspace-versions.md)), or a
snapshot per thread, could help later. An external agent is told the beginning again, with what
that loses, and the thread should show it. A thread is closed or archived, not deleted.

**A smaller step for the commonest case.** For a side question alone, less would do - as `/btw` in
Claude Code, which asks "a side question about the current session without adding to the
conversation" ([commands](https://code.claude.com/docs/en/commands.md), 2026-11-16): a person asks
one agent privately, with the channel's context; the answer is seen only by whoever asked and does
not enter the transcript. That needs neither a copy nor a channel. It does not replace threads, but
could come first.

**Open in this assessment:**

- Is a thread a `Channel` with an origin - one type, with a field naming its channel and the
  position in it - or a type of its own that shares the channel's machinery? Everything a channel
  does, a thread does too; what differs is where it is listed, what it takes from where, and the
  way back. One type looks simpler.
- Who takes part: every participant of the channel, those chosen by whoever opens it - and may an
  agent that is not in the channel, 558's sub-agent, take part?
- Who writes the result that flows back, and in whose name does it stand in the channel?

## Open questions

- Alternatives with one shown, or conversations side by side (*The question underneath*)?
- Who may branch: everyone who takes part, whoever may read the channel, or the owner?
- With B: copy or reference for the shared beginning, and for the agents' states?
- With B: does a branch take every participant along, or may the one who branches choose?
- How are external agents treated - given the beginning again, or left behind?
- May a branch be merged back, or is a branch only ever followed on its own?

## Decision

The maintainer's decision of 2026-11-16, on the assessment *Threads: a third level below the
channel*:

- **Conversations side by side, as threads.** The question underneath is answered: a branch in the
  sense of this issue is a conversation beside another, not an alternative shown instead of it.
- **A thread is part of its channel, a type of its own** - not a channel with an origin. What the
  channel has, its threads share: its participants - one who joins the channel later may take part
  in its threads too -, its notes, and its other properties. That is what tells a thread from a
  separate channel.
- **A thread may have a closed circle**, which limits who speaks in it, not who sees it: those of
  the channel's participants chosen for it speak there, a later newcomer to the channel does not
  join them, and everyone in the channel reads the thread all the same. Knowledge stays shared: the
  notes and the artifacts of a work are the whole channel's, a closed thread keeps no notes of its
  own, and nothing of its history - messages, attachments - is closed to the channel. Limiting what
  is seen would mean limiting the access to messages and attachments in the history as well, which
  is far too complex; whoever wants that strong a separation opens a channel of its own, which is
  what a channel is for.
- **Splitting a thread off into a new channel** dissolves exactly what it shares with its channel,
  and is a branch in the sense of B: a channel of its own with a copied beginning, its own
  participants and its own notes.

This departs from the assessment in two points: a thread is not a `Channel` with an origin, and
opening one copies no `AgentChannelState` as a whole, since the notes stay the channel's.

### What follows from it

In the model this is A - several conversations within one channel -, but as a concept of its own
and without alternatives: threads hang at messages and only grow. So A's costs above apply, and they
become the questions of the plan:

- **What an agent keeps per thread.** `AgentChannelState` splits into what is the channel's - notes
  and private notes - and what is a conversation's - its transcript and its turn state. A transcript
  is a chain, and a chain is a tree already: an agent's transcript in a thread could be a branch of
  its transcript in the channel, beginning where the thread was opened. Nothing is copied, and an
  agent that comes to the thread later begins there as well.
- **Whether the channel's own line is a thread too.** If a channel consists of threads, the first
  one without an origin, there is one kind of conversation, and nothing that reads one has to tell
  the channel's line from a thread; otherwise every such place has two cases. Recommended, not
  decided (*Proposal for threads*).
- **Who hears a thread.** Every participant of the channel may read every thread; an agent hears a
  thread only once it takes part in it, so the contexts of the others stay free of it. When an agent
  takes part - addressed by a mention, routed there by the moderator, or chosen by whoever opens the
  thread - is proposed under *Proposal for threads*.
- **Notes reach the whole channel**, whichever thread they were written in: wanted, since they are
  the circle's, and the one way besides a result in which a thread reaches the channel.
- **A closed circle is a rule of who speaks**, as the moderator applies it: only its members write
  in the thread, and only its agents are routed there. It needs no access list of its own, and
  every event of the thread reaches whoever reads the channel.
- **Every message and event names its thread**, and so do the REST API and the streams; the
  moderator routes per thread, mentions included.
- **An agent asked in two threads** answers one after the other (*Threads in the running
  channel*).
- **Compaction and the transcript window** ([380](380-transcript-window.md)) work per thread.
- **The way back.** A result handed back is a message in the line the thread hangs at.
- **A newcomer** sees of a thread what `HistoryAccess` gives them of the channel.
- **Splitting off** needs what B lists: a lineage, the copied beginning, the agents' transcripts of
  the thread copied into the new channel, a rule of who may split, a name. Whether the channel's
  notes are copied along or left behind is open.

The next step is a plan for threads in this issue, or issues of their own for its parts - threads,
splitting off, and a side question as `/btw`.

### Threads in the running channel

Decided by the maintainer on 2026-11-16:

- **No loop of its own.** Threads run in their channel's `ChannelLoop`; a `ThreadLoop` beside it
  would be far too heavy. It would need everything a channel's loop has - the moderator, the
  routing, an inbox, a history, the internal actors, the participants kept in step with the
  channel's -, and an agent would have a second actor in the same channel, a second writer of the
  channel's notes.
- **One actor per agent and channel, a head per thread.** The agent's transcript keeps a branch of
  its own, with a head of its own, for every thread the agent takes part in.
- **No turns at once across threads.** An agent works on one turn at a time, in whichever thread;
  a person cannot write in several threads at once either.
- **Threads can be unloaded.** A conversation can grow without bound - the history is read by
  pages, the transcript is bounded by compaction -, so the threads cannot all be held in memory
  for ever.

What stays as it is: the channel keeps one history, in which every entry names its thread, so that
an actor's inbox, where it stands (`heard`), catching up and starting again read one sequence per
channel, as today; the notes have one writer; and whoever joins the channel takes part in it as
before.

What changes:

- **`AgentChannelState` splits** into what is the channel's - notes, private notes, `heard`,
  `last_message`, the prompt texts and tool sets kept, the numbers of attachments, the tokens
  used - and a state per thread: the head of its branch, the window of it, its turn and
  `processing_state`, and how full its context is.
- **The thread of an event** is named in its `EventContext` and in the history's entry.
- **Every reader of the transcript in memory** - a turn, a compaction, how full the context is,
  answering again, taking in a change of a message - names the branch it reads; the window of
  [380](380-transcript-window.md) is held per thread.
- **Perceiving** appends a message said in a thread to the branch of that thread.
- **During a turn in a thread**, the agent takes ahead only what is said in that thread - the
  `EventReceiver` already takes envelopes of a kind ahead -, and what comes in other threads waits
  in the inbox. Then it works through the threads with requests open, by the oldest request not
  answered, and answers in each the last one, having heard everything before it: today's rule,
  per thread.
- **The web UI** shows where an agent is answering at that thread, so that one sees why it is
  silent elsewhere.

What is still hard:

- **A fork of the agent itself** ([558](558-no-agent-hands-a-task-to-a-sub-agent.md)): its turn
  waits in a tool call for a result that its own actor would have to produce in a thread - with
  one turn at a time, a deadlock. Such a fork runs in the background, or does not exist; a
  sub-agent that is another agent has no such problem.
- **A change of a message before a thread forked** ([234](234-modify-revoke-and-retry-messages.md))
  is appended behind it on every head whose path holds the message, not on one.
- **External agents.** The remote agent keeps one conversation; a thread needs a remote context of
  its own, a `RemoteState` per thread.

### Proposal for threads

The author's proposal of 2026-11-16 - not decided.

**The channel's line is a thread**: the one without an origin. What sets it apart follows from
that: it hangs at no message and has no way back, it is never closed and has no closed circle -
all of the channel's participants take part in it -, and it is unloaded as any thread is. So there
is one kind of conversation: whatever reads one - a turn, a compaction, the moderator, the history,
the REST API - knows threads alone, and `AgentChannelState` holds thread states, the channel's line
among them. The user sees nothing of it: in the web UI the line is the channel, and only what hangs
at a message is called a thread.

**Threads form a tree.** A thread's origin is a thread and a message in it; its branch in an
agent's transcript forks from the branch of its origin where it was opened, and its result goes
back to its origin. Its participants are the channel's, not bounded by those of its origin, since
what is seen is the whole channel's anyway; and it is unloaded on its own, wherever in the tree it
is. Limiting the depth would be the special case, and 558 needs nesting: an agent working in a
thread that starts a sub-agent opens a thread in a thread. Depth hurts in the presentation: the web
UI shows one thread at a time with its path - "channel › thread A › thread B" - instead of the
tree; whether users may nest is a decision of the product that changes nothing of the model. An
agent that comes to a thread without having taken part in its origin begins at its own branch in
the nearest thread above in which it takes part - at last the channel's line -, and is given the
origin's messages up to the fork to read, as a newcomer is: otherwise it does not know what the
message the thread hangs at is about.

**Who takes part in a thread.** A thread keeps who of the channel's participants takes part in it:

- when it is opened: whoever opens it, the author of the message it hangs at, and whom its first
  message mentions;
- later: a person by writing in it, an agent by being mentioned in it;
- in a closed circle, those fixed when it was opened, and nobody else: whoever is not among them
  cannot write there, and a mention of an agent outside brings nobody in - the moderator says so,
  as it tells its other notices.

An open and a closed thread so differ in one thing alone: whether who takes part may grow.

**Who hears and who answers.** An agent that takes part in a thread hears everything said in it -
listening silently, appended to its branch, without a call of its model; one that does not has no
branch of it. Who answers is the moderator's rule in a thread: the channel's `ResponderPolicy`,
which is told which agents could answer, is given those taking part in the thread, and the thread's
conversation. Mentions stay as they are; without one, the policy chooses among those taking part -
with one agent in the thread, under `round_robin_policy`, `broadcast_policy` or
`user_driven_policy`, that one: the usual case of a side question. The moderator keeps its context
(`context_messages`) and its count of hops between agents per thread. A policy of its own for
threads would be possible, but has no user yet.

**Unloading, as [381](381-inactive-channels-held-in-memory.md) unloads a channel.**

- **Closed** is a state the user sees: the thread is done - the natural moment to hand its result
  back -, and whoever writes in it opens it again. **Unloaded** is the runtime's alone, and nobody
  notices it: an open thread in which nothing happened for a while is unloaded, a closed one at
  once.
- **Unloading a thread** stores every agent's state of it, between turns, and lets go of the window
  of its branch - a turn is never cut off for it -; the moderator drops its context of the thread,
  as it starts empty after a restart. What stays is the thread's entry in the channel's list: its
  id, the message it hangs at, its circle, and whether it is open.
- **Idle** means as in 381: no message, request, approval or turn in the thread for a configured
  time. **Any event in the thread loads it again**: every agent reads its state of the thread and
  its branch from the window's start from the store - what a restart does today, for one thread.
- **An agent's state of a thread is a document of its own**, per agent, channel and thread, and no
  part of `AgentChannelState`'s, so that starting a channel does not load every thread. The history
  stays one per channel, but the store reads a page of one thread without searching the channel's.
  The list of a channel's threads is read by pages, the open ones first.
- **One notion of idle on three levels.** With the channel's line a thread, a channel whose threads
  are all unloaded is idle and is unloaded (381), and a session whose channels are all unloaded is
  unloaded. So unloading threads belongs to the epic [378](378-resource-management-epic.md), and is
  best designed together with 381 rather than as a second mechanism.

**An order of steps.**

1. Restructuring without a change of behaviour: the channel's line becomes a thread - the thread
   named in `EventContext` and in the history, `AgentChannelState` split into the channel's part
   and the thread states -, with one thread per channel.
2. Threads, with one turn at a time: opening one, who takes part, the moderator per thread, a head
   per thread, handing a result back.
3. Each on its own afterwards: closed circles, unloading (with 381), external agents in threads,
   splitting a thread off into a channel, and the side question as `/btw`.

### Decided on 2026-11-17

Decided by the maintainer, on the proposal above and on the sub-agents of
[558](558-no-agent-hands-a-task-to-a-sub-agent.md#decision):

- **The tree is the model's, not the user's.** Threads form a tree so that there is one concept,
  and the channel's line takes no special part: it is a thread, the one without an origin. That
  decides *The channel's line is a thread* above. No frontend shows the tree; the path of
  *Threads form a tree* - "channel › thread A › thread B" - is not wanted.
- **A person branches once.** A person opens a thread only at a message of the channel's line;
  deeper threads come only from sub-agents. The rule is the kernel's, where a thread is opened -
  not the TUI's or the REST server's, which offer nothing else and are refused if they ask for it.
- **A thread begins in one of two ways.** *Forked*: the agents' branches fork from where they
  stand in the thread's origin, and they know the conversation so far - as a person opens a
  thread. *Fresh*: the information and resources of the channel are shared - its notes, its
  artifacts, its properties -, but the conversation begins anew: every agent's branch in the
  thread begins at the root - an external agent's calls begin there already -, except the branch
  of whoever opened it, if it takes part, which forks, since it has to know why it asked. A
  sub-agent's thread is fresh.
- **A thread has a responder policy of its own**, chosen when it is opened - the channel's unless
  another is named -, since what suits a thread depends on what it is for; it chooses among those
  taking part in the thread. This replaces *the channel's `ResponderPolicy`* in *Who hears and who
  answers* above. A sub-agent's thread needs one of its own, since under the channel's
  `user_driven_policy` or `mentions_only_policy` nobody would be asked reliably.
- **Hops between agents are counted per thread**, from the thread's first message on - for a
  sub-agent, from its brief.

The fork of an agent itself, under *What is still hard*, is not needed: a sub-agent is a new agent
of a personality, and its start does not block the agent that started it (558).

## Plan: the first steps

The author's proposal of 2026-11-17, worked out with a second reviewing model - not decided. Put
up for decision are step 1 and the points D1 to D6 under *To decide*; step 2 is a sketch, to be
planned with 558.

### The cut: what 558 needs first

The first user of threads is [558](558-no-agent-hands-a-task-to-a-sub-agent.md#decision), and in
it **no agent holds two threads**: the main agent takes no part in the sub-agent's thread, and the
sub-agent, a new agent, takes part in its own thread alone. So neither a transcript with a head
per thread nor the split of `AgentChannelState` has a user before threads that people open, which
are forked and in which an agent of the line takes part as well. The plan therefore cuts narrower
than *An order of steps* above, which proposed both for the first step:

1. **Restructuring, without a change of behaviour**: every event and every entry of the history
   names the thread it happens in, and everything that goes by a thread knows one, the line.
2. **Threads for sub-agents**, together with 558: opening and closing a thread, taking an agent
   into it alone, delivering and reading by thread, the moderator per thread, and the frontends
   showing the line.
3. **Later, with the threads people open**: a transcript with a branch per thread, the split of
   `AgentChannelState`, who takes part growing by mention, a thread's own policy, unloading with
   [381](381-inactive-channels-held-in-memory.md), closed circles, splitting off.

Nothing in 1 and 2 is without a caller: 1 is the preparation of 2, as *Restructure, then extend*
asks, and the first caller of opening a thread is 558's tool.

### The pattern: threads are state of the channel

The closest thing the codebase has is how a channel keeps who takes part in it: the participants
are named in the channel's document, and when they took part is derived from the history - the
`ParticipantJoined` and `ParticipantLeft` entries, kept by the store beside the chain as the
`Attendance`. Threads would be kept the same way, as **state of the channel**, not as entities of
their own:

- a thread is opened, closed and opened again by entries of the channel's history
  (`ThreadOpened`, `ThreadClosed`), in the name of whoever did it;
- what the running channel needs of its open threads when it starts - the origin, the opener,
  later its policy - is a small part of `ChannelState`, recorded whole (`ChannelState::threads`),
  as `history_from` is. Every thread is an entry of it, the line included: the line's has no
  origin and is there from the channel's start, so that whether a thread is open, and how it is
  moderated, is read alike for every thread and nothing asks whether it is the line;
- which thread an agent takes part in is recorded with its join (`ParticipantJoined::thread`), the
  line for every join as today, and kept by the store in the `Attendance`;
- the list of all threads, closed ones too, is a page of the history's items of those entries,
  the open ones from the state first.

Weighed against a stored kind `Thread` that lies in its channel as a channel lies in its session
(`Stored`, `Within`, a `ThreadSpec`) - the shape first suggested in review. The state wins on
four points: a thread has one writer, the channel's loop (STORAGE.md, *one writer per state*);
it has no attributes of its own to keep in step with the channel's - access list, classification,
components -, since what it shares with the channel is all of them; it is purged with the
channel's state; and it adds no level to what [556](556-what-an-id-lies-in-is-found-by-searching.md)
has to find by id. A thread's own policy fits as well: a `ComponentConfig` in its part of the
state, built in the channel owner's name as the channel's components are.

### To decide

D1 to D6 were decided by the maintainer on 2026-11-17, as recommended, and D8 on the same day,
the maintainer's own proposal, its type named `Thread`; D4 was changed the same day, to no field of
its own (see *History*). Still to be confirmed: the
departure in step 2 from a sub-agent's thread having a policy of its own (*How a sub-agent is
asked*).

- **D1 - The cut above**: threads for 558 first, the head per thread and the split of
  `AgentChannelState` with the threads people open. Recommended.
- **D2 - Threads as state of the channel**, not as a stored kind. Recommended.
- **D3 - How the line is named.** Every thread has a `ThreadId`; the line's is derived from its
  channel's (`ThreadId::line(channel_id)`, the same UUID), so that an event made for a channel -
  `EventContext::new` - is in its line without its makers knowing threads, and the line needs no
  document, no search and no constant shared by every channel. The alternative is a field
  `Channel::line` set when the channel is built, which every maker of an `EventContext` would have
  to read. Recommended: derived.
- **D4 - What happens in a thread and what in the whole channel.** An entry of the history is of
  one thread or of the channel as a whole, by its kind (table below); the thread is a field of
  `Recorded` beside `sender` and `recipients`, stamped by the channel, `None` for the whole
  channel - not read out of `Recorded::context`, which `Recorded::new` builds from
  `EventContext::default()`, and which a store does not filter by. Recommended.
- **D5 - Agents hear by thread, users the whole channel.** An agent is delivered what happens in
  the threads it takes part in, and what happens in the whole channel; a user everything. The
  reason for treating them differently: what an agent hears enters its context and is paid for by
  every call after it, while what a user is delivered is shown by a frontend that sorts it by
  thread - and every participant may read every thread anyway (*Decision*). Recommended.
- **D6 - How the sub-agent is kept to its thread**, open in 558: by the rule of D5, not by access
  lists. Its join names its thread (`ParticipantJoined::thread`), its range in the `Attendance` is
  of that thread, and what it is delivered live, what it catches up with from the history
  (`Behind::recorded_for`, read as the participant) and its history as the REST API shows it are
  what happened in its thread and in the whole channel. This meets the decision that *every
  participant may read every thread* only for people: they are delivered everything. An agent has
  no read of its own channel besides what it is delivered - `read_history` reads other channels -,
  so the conflict does not arise yet; a tool that reads an agent's own channel would raise it
  again. Recommended.
- **D7 - The shape of a transcript with several threads** stays open until step 3; it is named
  below, since the head per thread is recorded as decided and the code argues for weighing it.
- **D8 - The channel's loop holds a thread object per thread, with a moderator of its own.** Not a
  loop of its own (*Threads in the running channel*), but a split of the code before the channel
  grows further: of what `ChannelLoop` holds, nearly everything is the whole channel's - the
  actors and their lifecycle, the inboxes and catching up, the history, admission and rights, the
  participants, the directory of mentions, `quiet`, how an envelope is stamped and delivered -;
  what is a thread's is its moderator, with everything the moderator keeps. So a thread object of
  the kernel (`channel::thread::Thread`; `ChannelThread`, set apart from `std::thread`, was the
  alternative) holds the thread's `ThreadState`, a moderator built for it from the channel's
  components, and who takes part in it; the channel admits an event, finds its thread by
  `EventContext::thread_id`, has the thread decide its `Routing`, and records and delivers it. The
  line is such a thread object too.

  What it makes simpler: the moderator's context (`PolicyModerator::heard`), what a policy keeps
  (`RoundRobinPolicy::next_agent_index`) and the count of hops are each a thread's without being
  keyed by one; a thread's own policy (*Decided on 2026-11-17*) is a moderator built from a
  `ComponentConfig` of the thread's instead of the channel's, with no mechanism of its own; and an
  unloaded thread is a thread object let go. What it costs: the rule that a channel has exactly
  one moderator (COMPONENTS.md, *Facets of which there is exactly one*) becomes one per thread;
  the components of a moderator are built once per thread, in the channel owner's name - for
  `decision_policy` a client of its model per thread -; and step 1 grows by the thread object,
  which has a user from the start, the line. An event of the whole channel - a join, `Fitted` - is
  routed by the thread its context names, the line unless made for another, and delivered by
  `Recorded::thread` being `None`. The rules over the order that read the history -
  `turn_given_up_last` - stay methods of the history, given a `ThreadId`. Decided.

### What is of a thread, and what of the channel

| Of the thread it happens in | Of the whole channel |
|---|---|
| `Said`, `Given`, `Withdrawn`, `Compacted` | `ParticipantJoined`, `ParticipantLeft`, `HistoryShared` |
| `TurnFinished`, `TurnGivenUp`, `PlanStepStarted`, `ActivityFinished` | `Fitted` |
| `CommandRequested`, `CommandExecuted`, `CommandFailed` | `Modified`, `Revoked`, `Reclassified` |
| `RetryRequested`, `RetryAccepted`, `RetryRefused` | `ThreadOpened`, `ThreadClosed` |
| `ResponseRequested`, `ChainCutOff`, `ResponderUndecided` | |
| `ApprovalRequested` to `ApprovalDecided`, `QuestionRequested`, `QuestionClosed` | |
| `EventRejected`, `EventRefused` | |

A change of a message is of the whole channel, as it is today of every branch: it reaches whoever
holds the message, and an agent whose transcript does not hold it records nothing (AGENT-TURN.md,
*Changes to a message*). An approval and a question are of the thread of the call that asks them.
A join and a leave of an agent taken into a thread alone are of that thread, not of the channel:
otherwise every agent of the line would note a sub-agent coming and going in its transcript
(`AgentChannelState::note_announced`) and find it among who is in the channel in its prompt.

### Rules that go by the order of the whole channel

A second thread breaks every rule that reads "the last" or "the next" over the whole channel. Each
becomes a rule over one thread in step 2:

- **Answering again** (`Chain<Recorded>::turn_given_up_last`, `RetryRefused::NotLast`): the last
  thing of the conversation in the answer's thread, not in the channel.
- **What a turn that ended answers** (`AgentBody::drop_answered`, "the very next event"): holds as
  long as an agent hears one thread; with several, step 3.
- **The moderator's context** (`PolicyModerator::heard`): the thread's, since every thread has a
  moderator of its own (D8).
- **What a policy keeps** (`ResponderPolicy` is public): `round_robin_policy` keeps whose turn is
  next (`RoundRobinPolicy::next_agent_index`), which requests in a thread would move on for the
  line if the moderator were shared; with a moderator per thread (D8), its policy keeps it for its
  thread alone.
- **Hops between agents** (`EventContext::agent_hops`): counted from the thread's first message,
  which begins at 0 (*Decided on 2026-11-17*).
- **A quiet channel after a cancel** (`ChannelLoop::quiet`): stays of the whole channel - a cancel
  cancels every agent of it. Changed on 2026-11-17 (*Plan: step 2*, 7): being quiet is a thread's,
  and a cancel ends the work of one thread or of the whole channel.
- **What is new to a user** (`announce_said`, `ReadMark`): counted in the line; what is new in a
  thread is shown at it, later.
- **Reading another channel** (`read_history`): reads the line of the other channel; its threads
  are not read from outside in step 2.

### Step 1: every event names its thread

No change of behaviour: there is one thread, the line, and every rule gives what it gives today.

```rust
// caelumora-domain/src/channel/thread.rs
/// A thread of a channel: a conversation of its own within it (issue 557).
pub struct ThreadId(Uuid);
impl ThreadId {
    /// The thread of a channel's own line: its id derived from the channel's.
    pub fn line(channel: ChannelId) -> ThreadId;
}

// caelumora-domain/src/actor/envelope.rs
pub struct EventContext {
    /// The thread it happens in; the line of its channel unless made for another. `next` keeps it.
    pub thread_id: ThreadId,
    // ...
}

// caelumora-domain/src/history/entry.rs
impl HistoryEntry {
    /// Whether it happens in one thread - a message said, a turn's end - rather than in the whole
    /// channel - a join, a change of a message, a classification.
    pub fn is_of_thread(&self) -> bool;
}

// caelumora-domain/src/history/recorded.rs
pub struct Recorded {
    /// The thread it happened in, `None` if in the whole channel (`HistoryEntry::is_of_thread`):
    /// what a store will filter by beside its sender and its recipients.
    pub thread: Option<ThreadId>,
    // ...
}
```

```rust
// caelumora-kernel/src/channel/thread.rs (D8)
/// A thread of a running channel: what decides whom its events go to and who is asked in it.
pub(crate) struct Thread { /* its id, its `ThreadState` from step 2, its moderator */ }
impl Thread {
    /// The thread `id`, moderated by `moderator`, built for it.
    pub(crate) fn new(id: ThreadId, moderator: ComponentImpl<dyn Moderator>) -> Self;
    /// What becomes of `event`, which happens in this thread (`Moderator::route`).
    pub(crate) async fn route(&self, event: &Envelope, context: &SecurityContext) -> Routing;
    /// `actor` takes part in the thread: its moderator may ask it.
    pub(crate) fn add_actor(&self, actor: Arc<dyn Actor>);
    /// `actor` no longer takes part in the thread.
    pub(crate) fn remove_actor(&self, actor: ActorId);
}
```

`ChannelLoop` holds its threads by id - in step 1 the line alone - in place of its one
`moderator`, and routes every event by the thread `EventContext::thread_id` names (D8). Every
literal of an `EventContext` names its thread; the compiler finds them.

Files: the domain's `channel`, `actor/envelope.rs`, `history/entry.rs` and `history/recorded.rs`;
the kernel's `channel` - `mod.rs`, `run.rs`, `participants.rs` where actors are added to and
removed from the moderator, and the new `thread.rs`; the stores of histories, which keep
`Recorded::thread` - the `MemoryStore`, `JsonStates` and Postgres, there as a column of the
history's entries -, to filter by it from step 2; the REST API's types, the REST client, the web
UI and the TUI, which are given `thread_id` in every envelope and ignore it; COMPONENTS.md and
CHANNELS.md, for a moderator per thread.

Tests: the existing suites, unchanged; one each that an event made for a channel is in its line,
that `next` keeps the thread, that a store keeps and returns an entry's thread, and that the
channel routes an event by its thread's moderator.

### Step 2: threads for sub-agents, with 558 - a sketch

A sketch, not yet an interface to decide: it rests on parts not yet read for this plan -
[CHANNELS.md](../docs/CHANNELS.md#participants) on taking someone in,
[PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md), [APPROVALS.md](../docs/APPROVALS.md) and
the rest of [STORAGE.md](../docs/STORAGE.md#histories) on items -, and it is planned with 558.

```rust
// caelumora-domain/src/history/entry.rs
pub enum HistoryEntry {
    /// `by` opened the thread `thread` at `at`, at the message `origin.message` of the thread
    /// `origin.thread`: from here on what is said in it is said there.
    ThreadOpened { thread: ThreadId, origin: ThreadOrigin, by: ActorId, at: DateTime<Utc> },
    /// `by` closed the thread `thread` at `at`: nothing more is said in it.
    ThreadClosed { thread: ThreadId, by: ActorId, at: DateTime<Utc> },
    /// ... `ParticipantJoined` gains `thread: ThreadId`: the thread it takes part in, the line
    /// for whoever joins the channel as today.
}

// caelumora-domain/src/channel/thread.rs
/// Where a thread hangs: a message of another thread of the channel.
pub struct ThreadOrigin { pub thread: ThreadId, pub message: MessageId }
/// A thread as the running channel keeps it: where it hangs and who opened it, `None` for the
/// line, which hangs nowhere.
pub struct ThreadState { pub opened: Option<(ThreadOrigin, ActorId)> }

// caelumora-domain/src/channel/state.rs
pub struct ChannelState {
    /// The threads open in the channel, the line among them, recorded whole: what it needs of
    /// them when it starts.
    pub threads: BTreeMap<ThreadId, ThreadState>,
    // ...
}

// caelumora-domain/src/history/filter.rs
impl Filter<HistoryField> {
    /// The items of the thread `thread` and of the whole channel: a page of one thread.
    pub fn in_thread(thread: ThreadId) -> Self;
}

// caelumora-kernel/src/channel/mod.rs
impl ChannelLoop {
    /// Opens a thread at `origin` in the name of `context`, and returns its id.
    pub(crate) async fn open_thread(&self, context: &SecurityContext, origin: ThreadOrigin)
        -> Result<ThreadId, ChannelError>;
    /// Closes the thread `thread` in the name of `context`; `false` if it is not open.
    pub(crate) async fn close_thread(&self, context: &SecurityContext, thread: ThreadId)
        -> Result<bool, ChannelError>;
}

// caelumora-store/src/history.rs
/// The `Attendance` keeps each range with the thread it is of; delivering, catching up and an
/// agent's history go by it (D5, D6).
```

What else changes, each where the rule lives:

- **Delivering** (`Envelope::recipients`, `ChannelLoop::deliveries`): an agent is delivered an
  envelope of a thread only if it takes part in that thread, and every envelope of the whole
  channel; a user everything. Whether an entry is of a thread or of the whole channel is
  `HistoryEntry::is_of_thread`, after the table above; its thread is the one its context names
  (D4 as changed on 2026-11-17).
- **Reading** (`Recorded::seen_by`, `Filter::seen_by` and every store of histories): the same rule
  over the reader's ranges and the entry's thread.
- **The moderator**: each thread's knows the agents taking part in it (D8), its `ResponderPolicy`
  is given them and the thread's conversation, and a mention of an agent that takes no part asks
  nobody.
- **How a sub-agent is asked.** The brief and every later message of the main agent mention the
  sub-agent, and a mention asks it under every policy. That replaces, for the first version, the
  policy of its own that *Decided on 2026-11-17* says a sub-agent's thread needs - a departure
  from the decision, to be confirmed: its reason, that the channel's policy may ask nobody, is met
  by the mention, and a thread's own policy comes with a use that needs one - with D8 a moderator
  built from the thread's own components.
- **Who may open a thread** is the kernel's rule (*Decided on 2026-11-17*): in step 2 only the
  runtime of an agent opens one, for a sub-agent; a person cannot yet, so nothing of the
  frontends opens one. Which action opening asks in whose name is open.
- **Taking the sub-agent in.** A channel takes in only participants of its session (CHANNELS.md,
  *Building a channel*), so the sub-agent joins the session as well; its join and its leave are of
  its thread (table above).
- **`ThreadOpened` and `ThreadClosed` are items** of the history, so that a page filtered by them
  lists the threads.
- **What is sent into a closed thread** is not let in, with a `Rejection` of its own - presumably
  `ThreadClosed` -, worded for its sender in the web UI and the TUI: the case a person meets once
  threads are closed. Step 1 rejects what names a thread the channel does not have as `Invalid`,
  which only a context not made by the channel can do (decided on 2026-11-17).
- **The REST API** reads a page of one thread (`?thread=`, with `Filter::in_thread`; the line
  without it). The web UI and the TUI show the line, and of another thread its approvals and its
  questions as today, since its calls wait for them; its messages are left out of the line's view,
  and a view of a thread comes later.
- **The rules over the order** as listed above.

Tests: in the kernel's channel tests, with a scripted agent - an agent taken into a thread alone
is delivered what is said in it and in the whole channel, and nothing of the line; the line's
agents nothing of the thread; the users all of it; a page of a thread holds its items alone; the
moderator asks only those taking part; a closed thread takes nothing more; the threads open are
there again after a restart. In the security suite, opening and closing a thread, in whose name.

### Open for 558, before step 2

- **An answer taken away.** An agent's turn answers every request taken in together, in the
  context of the last (AGENT-TURN.md, *Between turns*). If a user writes in the sub-agent's thread
  before it answers the brief, the turn answers in the user's context, and the answer is not the
  main agent's to be given. Recommended for the first version: the thread of a sub-agent is a
  closed circle - only the main agent's messages, through its tools, are said there.
- **How the answer is given.** Giving to read happens today only within a turn of the agent
  (`Turn::give`), and the way another participant gives an agent something is an agent command
  (`/skill`): recorded, delivered again from the history, and admitted again. The way back would
  either be such a command asked of the main agent, or a new event its intake takes in as given
  between turns. To be weighed with 558's plan.

### Later: the shape of a transcript with several threads (D7)

Recorded as decided: the agent's transcript keeps a branch with a head of its own for every thread
(*Threads in the running channel*). Two shapes would do it, and unloading - decided as well -
tells them apart:

- **One chain, a head per thread.** The store keeps it as now, and `TranscriptStore::path` finds
  the context of a message on whichever thread's branch. But what is held in memory is a suffix of
  the chain from one position on (`Chain::keep_from`, `transcript_from`), and the branches of
  several threads interleave by position: a thread that stays loaded keeps everything after where
  its window begins in memory, an unloaded thread's entries cannot be let go on their own, and
  loading one back means entries before what is held - which `Chain::apply` and `Chain::suffix`
  do not take.
- **A chain per thread**, the first entry of a forked one following an entry of its origin's
  chain - a suffix takes an entry that follows one it does not hold already. Every thread has its
  own window, head and positions, and is unloaded by letting its chain go. But
  `TranscriptStore::path` has to follow a path into the origin's chain, and the store keeps a
  transcript per agent, channel and thread.

To be decided with step 3; a fresh thread - all that 558 needs - is the same in both: a branch
that begins at the root.

### Stored data to delete

Step 1 adds a field to `EventContext` and to `Recorded`, without compatibility code
([CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md), *Stored data*): the state of every channel -
its history - and of every agent in a channel, whose turn under way keeps the `EventContext` it
answers (`Work::event_context`), have to be deleted, and the open tasks of external agents with
them. Step 2 changes what is stored again - `ParticipantJoined`, the `Attendance` and
`ChannelState` -, and the same states have to be deleted then.

## Plan: step 2, threads as building blocks

The author's proposal of 2026-11-17, after the sketch of step 2 was traced through the code - not
decided but where it says so. It replaces the sketch where they differ. The sub-agents themselves
are planned in 558,
[*Plan: the crate caelumora-subagents*](558-no-agent-hands-a-task-to-a-sub-agent.md#plan-the-crate-caelumora-subagents).

### What the code showed

Traced on 2026-11-17, with three investigations of the code:

- **Only an actor speaks in its own name.** `OriginEventSender` lets an actor send only as itself,
  and nothing outside `ChannelLoop::run` reaches the channel's input. A brief "in the main agent's
  name" said through the main agent's actor is a message of its turn: in its transcript, carrying
  its `turn_id`, and asking someone only at the end of that turn, in the line.
- **A join is applied after it is taken in** (`ActorChange::Start`, handled after the events by
  `run`'s biased select): what is said right after taking someone in can be routed before its
  inbox opens, and is never delivered to it.
- **No actor knows its thread.** `AgentBody::own`, the channel's announcements of a join or a leave
  (`ChannelLoop::announce`) and what a user addresses to an agent - an answer to its question, a
  request to answer again, a command, a nudge - are made in the line.
- **A mention of an agent that takes no part in the thread** is not addressed to it: the thread's
  policy chooses instead (`PolicyModerator::mentioned_responders`).
- **Quiet after a cancel is the whole channel's** (`ChannelLoop::quiet`): nobody is asked in any
  thread, and every actor's turn is cancelled.
- **A filter of one thread needs no column**: a page can be filtered by the kept item's
  `context.thread_id` and the entry's kind in the memory, the JSON and the Postgres store alike.

### Decided

The maintainer, on 2026-11-17: the channel offers building blocks - opening a thread, taking
someone into it - and knows nothing of sub-agents; what makes a sub-agent and carries its work lies
elsewhere, in a crate of its own, `caelumora-subagents` (558).

### The building blocks

Proposed:

1. **Founding a thread.** `ChannelLoop::open_thread` opens a thread at an origin and takes its
   founders into it, as founding a channel takes in its founders (`ChannelLoop::found`): naming
   them was opening the thread, so it asks no `UpdateChannel`. It asks a new action, `OpenThread`,
   granted to whoever reads the channel, as `CancelChannel` and `WakeChannel` are, and the founders'
   clearance for the channel's classification. `close_thread` closes it.

   ```rust
   /// Opens a thread at `origin`, in the name of `context`, with `founders` taking part in it
   /// alone, and returns its id.
   pub async fn open_thread(&self, context: &SecurityContext, origin: ThreadOrigin,
       founders: Vec<Participant>) -> Result<ThreadId, ChannelError>;
   /// Closes `thread` in the name of `context`, its delegate with it; `false` if it is not open.
   pub async fn close_thread(&self, context: &SecurityContext, thread: ThreadId)
       -> Result<bool, ChannelError>;
   ```

   A founder is taken into the session, then into the channel, taking part in this thread alone.
   Opening goes through `run`'s ordered input of changes: the thread, then its founders started,
   then whatever is said in it first.
2. **Giving something to read.** A new event by which an actor gives another participant a message
   to read and asks it to respond - recorded by the channel to the recipient by name, as a request
   to respond is, so that it is delivered again from the history (issue 223), and taken in by the
   recipient's intake as a `Given` entry of its transcript, with the giver named as `/skill` names
   one ("@researcher gave you this to read:"). The agent records the given message once, as it
   records what it is given today (`MessageGiven`); the message given has an id of its own. Nobody
   speaks in another's name: the giver is a statement of the event. Admitted from the delegate (5)
   of the thread it is given in, or given into the origin of the delegate's own thread.
3. **Threads in the running channel.** `ChannelLoop::threads` changes while the channel runs, and
   the channel's threads, their origins and their delegations are a part of `ChannelState`,
   recorded whole, from which the channel builds them as it starts.
4. **An actor knows its thread.** It learns from its runtime the thread it takes part in; what it
   makes itself - `own()` - happens there, and so do the channel's announcements of its join and
   leave. The thread of a join is the one its context names, as D4 was changed: no field of
   `ParticipantJoined`. The `Attendance` keeps each range with its thread; delivering, catching up
   and an agent's history go by it (D5, D6).
5. **Delegating.** A participant - the giver - has another - the recipient - work on something in
   a thread, and is given back what came of it in the thread's origin. A **delegate** carries it
   out: an internal actor of the thread, built by the kernel as the approver is
   (`ChannelLoop::create_actor`), no participant, of a kind of its own (`ActorId::Delegate`). It
   gives the recipient the brief to read (2), naming the giver, and asks it to respond; it holds
   the one brief that is open, and takes the recipient's next turn end in the thread as its answer;
   and it gives the giver, in the origin, what came of it - the answer, that the turn ended without
   one, that it was given up and why, or that the recipient was not asked - and asks the giver to
   respond. A further brief waits until the open one is answered. The giver is whom the caller acts
   for: nobody delegates in another's name. The delegate's giver, recipient, origin and open brief
   are the thread's part of `ChannelState`, written by the channel alone, and it is built again
   from there as the channel starts.

   ```rust
   /// Has the delegate of `thread` give `recipient` `brief` to read on behalf of whom `context`
   /// acts for, and bring what came of it back to them in the thread's origin.
   pub async fn delegate(&self, context: &SecurityContext, thread: ThreadId, recipient: ActorId,
       brief: String) -> Result<(), ChannelError>;
   ```

   Asked: that the giver reads the thread, and that the recipient takes part in it. The first
   delegation in a thread starts its delegate. Sub-agents are one user of it (558); an agent asking
   another agent of the channel in a thread aside, or a side question as `/btw`, could be others.
6. **Reaching a running channel from a plugin.** A plugin's writer is bound with the work's
   `SessionView` (`KernelBuilder::extend_writers`); the view gives out the running session it holds
   (`SessionView::session_loop`, today the crate's), and the plugin reaches the channel as a
   frontend does (`SessionLoop::channel_for`), acting in the work's context through the loop's
   public operations, each checking itself. The alternative is a service of the kernel's for
   acting on channels.
7. **Cancelling and waking a thread.** Decided by the maintainer on 2026-11-17 (558): a cancel
   ends the agents' work in a scope - one thread, or the whole channel. Being quiet becomes the
   state of a thread, held by its `Thread` (D8), no longer the channel's (`ChannelLoop::quiet`): a
   cancel of a thread ends the turns of the actors taking part in it and makes it quiet; a cancel
   of the channel does so for every thread, as a cancel does today. Waking goes the same two ways.
   Both ask what cancelling and waking ask today (`CancelChannel`, `WakeChannel`): a thread is part
   of its channel. A delegation into a quiet thread is refused, not waking it, so that a giver
   cannot undo a user's cancel; what the delegate gives back into a quiet thread is delivered, and
   nobody there is asked until it is woken. The interface is agreed with 2b.

### To decide in step 2

- **T1 - Founding a thread** instead of taking someone in with `UpdateChannel`, under a new action
  `OpenThread` granted by reading the channel. Recommended.
- **T2 - How a plugin reaches a running channel**: through `SessionView::session_loop`, or a
  service of its own. Recommended: the view.
- **T3 - `ActorId::Delegate`** as a new kind of actor, beside the approver's. Recommended.
- **T4** - where an internal actor of a plugin keeps its state - fell away with the delegate in the
  kernel: its state is the channel's.
- **T5 - The event of giving to read**: its name - `GivenToRead`, apart from the agent's own
  `MessageGiven` -, and whether the users see it - the brief and the answer given are what a person
  follows of a sub-agent. Recommended: `GivenToRead`, published to the users, given to the recipient
  by name.
- **T6 - What is of the whole channel**: by the entry's kind and the line's context, written into
  the filter, or a field derived as the kept item's `published` is (`KeptItem::thread`), so that
  the kinds are listed once (`HistoryEntry::is_of_thread`). Recommended: the derived field.

### The cut

1. **2a, restructuring without a change of behaviour**: threads built from `ChannelState` and
   changing at runtime, an actor knowing its thread, the thread of a join from its context,
   delivering and reading by thread - with everyone in the line, so that every rule gives what it
   gives today.
2. **2b, the building blocks with their first user**: founding and closing a thread, giving to
   read, delegating, cancelling and waking a thread, reaching a running channel - together with
   `caelumora-subagents` (558), which uses them, in commits each reviewed on its own. Ending a
   sub-agent needs letting go from a session first (issue
   [590](590-a-session-lets-nobody-go.md)).

## Done when

- The model is decided - within one channel, as channels, or not at all - with the reasoning
  recorded here.
- The follow-up is named: a plan in this issue, or issues of their own for its parts.

## History

- **2026-11-11** — Opened from the maintainer's new idea: branching only at the current point,
  every participant answering on any branch, and the consideration - not a decision - of modelling
  a branch as a channel. Takes up the part of [08](08-retry-and-edit-past-answers.md) that its
  rejection left without an issue.
- **2026-11-11** — How Claude Code branches added for comparison (*How Claude Code does it*), from
  its documentation: a branch there is a new session with a copied beginning, option B with a copy.
- **2026-11-16** — Threads assessed from the user's side (*Threads: a third level below the
  channel*): conversations side by side, as a third level below session and channel, with a way
  back by a result; one concept with the sub-agents of 558. Not decided.
- **2026-11-16** — Decided by the maintainer (*Decision*): conversations side by side, as threads -
  a type of its own, part of its channel, sharing its participants, notes and properties; splitting
  a thread off into a new channel is a branch; a closed circle of a thread limits who speaks, not
  who sees, and keeps no notes of its own. The consequences for the model are listed under *What
  follows from it*; the plan is still to be made.
- **2026-11-16** — Decided by the maintainer (*Threads in the running channel*): threads run in
  their channel's loop, with one actor per agent and a head of its transcript per thread, no turns
  at once across threads, and threads can be unloaded. Proposed, not decided (*Proposal for
  threads*): the channel's line as a thread, a tree of threads, who takes part in a thread and who
  answers there, unloading as 381 unloads a channel, and an order of steps.
- **2026-11-17** — Decided by the maintainer (*Decided on 2026-11-17*): the tree of threads is the
  model's alone, with the channel's line a thread; a person branches once, by a rule of the
  kernel; a thread begins forked or fresh; every thread has a responder policy of its own; hops are
  counted per thread. The sub-agents of 558 decided with it.
- **2026-11-17** — A plan for the first steps proposed (*Plan: the first steps*), worked out with a
  second reviewing model: threads for 558 first, as state of the channel, the line's id derived
  from its channel's, every entry of the history of a thread or of the whole channel, agents
  hearing by thread. Step 1 and the points D1 to D6 are put up for decision, step 2 is a sketch to
  be planned with 558, D7 stays open until step 3; not decided.
- **2026-11-17** — D1 to D6 of the plan decided by the maintainer, as recommended: threads for 558
  first, as state of the channel, the line's id derived from its channel's, every entry of a thread
  or of the whole channel, agents hearing by thread, the sub-agent kept to its thread by that rule.
  Asked in passing: an agent has one instance per channel, not per thread.
- **2026-11-17** — D8 added and decided, the maintainer's proposal: the channel's loop holds a
  thread object per thread, with a moderator of its own, as a split of the code before the channel
  grows further - step 1 introduces it, with the line as its first. The name of its type is open.
- **2026-11-17** — The type of D8 named `Thread` by the maintainer.
- **2026-11-17** — Step 1 implemented, without a change of behaviour for every event the program
  makes: `ThreadId` with the line's id derived from its channel's, `EventContext::thread_id`, kept
  by the history in the context of every entry, and the kernel's `Thread` with a moderator of its
  own, which `ChannelLoop` holds for its line and routes every event by. Where it departs from the
  plan: `Recorded::thread` and `HistoryEntry::is_of_thread` are not there (below); `Thread::new`
  takes no id, which nothing would read; and an event naming a thread the channel does not have
  is not let in (`Rejection::Invalid`, checked first in the channel's admission, before
  attachments are claimed), so that its sender is told - in the thread it named, as a rejection
  is said in the thread of what it rejects (`replying_to`), reaching its sender by name -, while
  one that enters all the same is dropped. The tests that forged envelopes in no channel make them
  in the channel's line now (`ChannelLoop::event_context`, `ChannelLoop::record_said`). Stored
  data to delete: the state of every channel, whose every entry keeps the context it came in; of
  the agents' states in a channel, those with a turn under way or an external agent's open task,
  which keep the context they answer (`Work::event_context`) - the others load as they are, and
  deleting them would lose their notes.
- **2026-11-17** — D4 changed by the maintainer: `Recorded` keeps no thread of its own. The thread
  of an entry is the one its context names, as its channel and session are, if the entry is of a
  thread rather than of the whole channel; a field beside them would repeat the context, and is
  no column of a store either - `sender` and `recipients` are none. That rule, and with it
  `HistoryEntry::is_of_thread` after the table above, come with their first user in step 2
  (*Delivering*, *Reading*).
- **2026-11-17** — Decided by the maintainer: in step 1 an event naming a thread the channel does
  not have stays rejected as `Rejection::Invalid`; a rejection of its own, for what is sent into a
  closed thread, comes with step 2 (*Step 2*, *What is sent into a closed thread*).
- **2026-11-17** — The sketch of step 2 traced through the code; the maintainer decided that the
  channel offers building blocks alone, and that the sub-agents' work lies in a crate of its own,
  `caelumora-subagents` (558). Proposed (*Plan: step 2, threads as building blocks*): founding and
  closing a thread, giving something to read, threads in `ChannelState`, an actor knowing its
  thread, kinds of internal actors registered by plugins, and reaching a running channel from a
  plugin; T1 to T6 to decide, and a cut into 2a and 2b.
- **2026-11-17** — Decided by the maintainer: the delegate is a concept of the kernel, not of the
  plugin - delegating, a building block of the channel (*Plan: step 2*, 5). The kinds of internal
  actors registered by plugins fell away with it, and so did T4; T3 became `ActorId::Delegate`.
- **2026-11-17** — T1, T2, T3, T5 and T6 decided by the maintainer, as recommended: founding a
  thread under `OpenThread`, granted by reading the channel; a plugin reaches a running channel
  through `SessionView::session_loop`; `ActorId::Delegate`; `GivenToRead`, published to the users
  and given to the recipient by name; and a derived `KeptItem::thread` for what is of the whole
  channel.
- **2026-11-17** — The interface of 2a agreed with the maintainer: `HistoryEntry::is_of_thread`
  and `Recorded::thread` with their first users; `KeptItem::thread`, derived as `published` is;
  the `Attendance` keeping each range with the thread its join's context names (`Attended`); one
  rule of reaching for delivering and reading (`Recipients`), by thread for agents; an actor
  knowing its thread (`ActorRuntime::thread_id`), and its join and leave announced there. Moved to
  2b: `ChannelState::threads` and threads changing at runtime, which have no user while the line
  is the one thread. External agents are covered alike, through the `AgentBody` both kinds share.
- **2026-11-17** — Step 2a implemented, without a change of behaviour: `HistoryEntry::is_of_thread`,
  `Recorded::thread`, `Envelope::thread` and `KeptItem::thread`; the `Attendance` keeping each
  range with its join's thread (`Attended`); one rule of reaching (`Recipients::goes_to`,
  `reaches`) by which the channel delivers and a store reads, by thread for agents; a participant's
  thread (`ResolvedParticipant::thread`), decided once as its actor starts and handed to its
  runtime (`ParticipantRuntime::thread_id`, `ActorRuntime::thread_id`), which its actor speaks of
  itself in (`Answering::own`), whose moderator alone may ask it, and where its join and leave are
  announced (`ChannelAnswering::in_thread`, `Leaving`). Where it departs from the agreed
  interface: a participant's thread is its channel's line, derived, not read from the `Attendance`
  as the channel starts - which thread an actor starts again in comes with 2b, with
  `ChannelState::threads`. The test helpers that recorded joins
  and messages in no channel's thread record them in the channel's line
  (`channel::testing::founded`, a test of the views). Stored data: the `Attendance` changed its
  form; the histories' derived parts have to be rebuilt (`HistoryStore::rebuild`) or the states of
  the channels deleted.
- **2026-11-17** — Decided by the maintainer, on a question of the review of 2a: a join, a leave
  and what a newcomer was given (`HistoryShared`) happen in the thread they join, not in the whole
  channel, so that the agents of the line do not hear a sub-agent come and go. Of the whole channel
  are a change of a message and a classification of the channel alone. This changes the table of
  *What is of a thread, and what of the channel*, whose note already said so of an agent taken into
  a thread alone; `HistoryEntry::is_of_thread` follows it.
- **2026-11-17** — Decided by the maintainer, with the open points of 558: a cancel ends the
  agents' work in one thread or in the whole channel, being quiet a thread's state, held by its
  `Thread`, and waking goes the same two ways (*Plan: step 2*, 7). The rule of *Rules that go by
  the order of the whole channel* that kept being quiet the whole channel's changes with it. Part
  of 2b; ending a sub-agent needs letting go from a session first, issue 590.
