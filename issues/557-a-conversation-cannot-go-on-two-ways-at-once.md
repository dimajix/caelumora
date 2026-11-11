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

## Open questions

- Alternatives with one shown, or conversations side by side (*The question underneath*)?
- Who may branch: everyone who takes part, whoever may read the channel, or the owner?
- With B: copy or reference for the shared beginning, and for the agents' states?
- With B: does a branch take every participant along, or may the one who branches choose?
- How are external agents treated - given the beginning again, or left behind?
- May a branch be merged back, or is a branch only ever followed on its own?

## Done when

- The model is decided - within one channel, as channels, or not at all - with the reasoning
  recorded here.
- The follow-up is named: a plan in this issue, or issues of their own for its parts.

## History

- **2026-11-11** — Opened from the maintainer's new idea: branching only at the current point,
  every participant answering on any branch, and the consideration - not a decision - of modelling
  a branch as a channel. Takes up the part of [08](08-retry-and-edit-past-answers.md) that its
  rejection left without an issue.
