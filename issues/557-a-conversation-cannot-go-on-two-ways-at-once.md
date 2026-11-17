# 557 — A conversation cannot go on in two directions at once

**Status:** open
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
