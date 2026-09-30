# 163 — There is no model to navigate from a session to what is in it

**Status:** open
**Area:** Architecture

## The problem

What the system holds is a graph: a session has channels, a channel has
members, an agent as a member has a history, a transcript, notes, private
notes and turns it gave up; an agent has a memory. The data refers along
it: a message to its context (`ContextRef`), a context to what was read
(`KnowledgeRef`, into other channels and the memory), a note to the turn
that wrote it (`after`), a memory entry to the channel it was written in
(`MemoryOrigin::channel`).

The code has no model of that graph. `caelumora-model` was meant to hold
it, but its types mostly hold the ids of their children (`Session::channels`
is a `Vec<ChannelId>`), and every step from one to the next is put
together anew, by whoever needs it, with its own conditions:

| What | How it is read today | Condition |
|---|---|---|
| Session, channel, members | `SessionManager::session_view` / `channel_view` (issue 80) | running or not, starts nothing |
| History | `open_channel` → `ChannelLoop::visible_history` | starts the session |
| Published notes | `open_channel` → `ChannelLoop::published_notes` | starts the session |
| A message's context, given-up turns | `open_channel` → `actor_by_id` → `Actor::message_context` / `given_up_turns` | the agent must be in the channel now, though `readable_state` reads from the store anyway |
| Transcript, memory, private notes | not at all for a user; only through resolving a context | – |
| The same for an agent | `SessionReader`: a second facade with its own lookups and checks, by channel *name* | – |

Consequences seen so far:

- An agent removed from a channel takes the context of its past messages
  with it: the actor is gone, the state is still in the store.
- Reading starts a session; describing it does not.
- Following a reference - "what did the agent see when it wrote this
  note", "which channel does this memory come from, and what was said
  there" - is possible in principle but written nowhere; one works it out
  in one's head each time.
- `MemberView::can_read_context` announces an edge (to the member's
  context) that the view cannot follow.

## Why it matters

- One model for reading, used by the runtime, the components and the REST
  server alike, instead of four entry points with different conditions.
- The access rule for going from one thing to the next would sit in one
  place, at that step, instead of in every facade again.
- The REST server could follow the model instead of assembling reads from
  the session manager, the channel loop and the actors - large potential
  for simplifying it, and the web UI would follow.

## Considerations

From a discussion with the maintainer (2026-09-30); nothing decided yet.

- **Navigation is what is missing.** The views are nodes; the edges
  between them are missing. Primarily an internal model - the REST API
  and the UI follow it.
- **Naming.** The maintainer's proposal: today's views (`SessionView`,
  `ChannelView`, `MemberView`, issue 80) become *snapshots*, and *views*
  is the name for the navigation. `SessionReader` is renamed as well - the
  suffix `Reader` is unwanted; it would be the navigation with the agent as
  viewer, starting in its channel.
- **Snapshots stay snapshots.** Issue 80 made a view a picture of what to
  show and offer, deciding nothing: an operation checks its own permission.
  The navigation should not blur that - a snapshot stays data; stepping
  along an edge is what asks the `AccessQuestions`.
- **Viewer.** A user or an agent; both walk the same edges with their own
  `SecurityContext`. That is the alignment of `SessionReader` with the
  frontends' reads that the guidelines ask for.
- **Where it lives.** Resolving an edge needs the stores and the
  `SecurityManager`, so the navigation belongs to the runtime; the model
  keeps the data and the references. Open whether the model should hold
  more of the structure itself, as was once intended.
- **Stored, not running.** The graph is what is in the stores; a running
  loop is a runtime matter, not another view of the data. Agent channel
  states are written after every step; whether a running channel's history
  in the store is always current is to be checked.
- **Points in time.** "The notes of agent X in channel Y" is ambiguous:
  now, or as the agent saw them when it produced message Z? The digests in
  a `ContextRef` are exactly such points; the chains can answer a prefix
  by digest. The navigation needs both, the current state and a state at
  a digest.
- **Edges into nothing.** Memory outlives the channel it points to, given-up
  turns are dead branches (issues 08, 145). The model has to say that an
  edge leads nowhere any more, not hide it in an error.
- **Branches.** History and transcript are trees (issue 08); "the history"
  is "the history on a branch".
- **Perspective.** The same message looks different to the agent's owner
  and to others (reasoning visibility, issue 61): nodes are not only
  filtered by viewer but shaped by it.
- **Context resolution.** `ContextResolver` resolves a `ContextRef` against
  one `AgentChannelState`; in the graph it is an edge from a message to its
  context, and `KnowledgeRef`s are edges on into other channels or the
  memory - answering "what did the agent see, and where did it come from"
  across channels.
- A note's `after` with its channel and agent leads to the history entry
  of the turn's last message and so to its `ContextRef`; that would make
  `NoteOrigin::transcript` (issue 92) redundant. Not for a turn that
  produced no message.

## Done when

To be decided with the design: at least, the reads of the REST server and
of `SessionReader` go through one navigation, with the permission asked at
each step, for running and stopped sessions alike.

## History

- **2026-09-30** — Opened from the discussion after
  [92](92-ledger-for-notes-and-memory.md). Related:
  [80](80-stored-channel-response.md) (the views),
  [17](17-information-sharing-within-session.md) and
  [128](128-actor-runtime-in-components.md) (`SessionReader`),
  [145](145-ended-turns-leave-a-record.md) (given-up turns),
  [08](08-retry-and-edit-past-answers.md) (branches).
- **2026-09-30** — Discussed: whether the views of issue 80 are still
  needed once there is navigation - probably not as a layer of their own.
  They do four things: carry data, filter what the viewer may read, answer
  access questions ahead (`can_change`, `can_read_context`), and look alike
  for running and stopped sessions. The navigation covers each: the data
  is the node's, loaded when it is stepped onto; filtering is the edge
  (`session.channels()` yields only readable channels); the questions
  become questions of the node (`channel.may_change()`,
  `member.may_read_context()`), asked by the node and not by its caller -
  which keeps what issue 80 was after, since its problem was the REST
  server choosing and asking the questions itself; and the difference
  between running and stopped goes when the graph reads from the stores.
  What is left of a view today is mostly the step before a REST response,
  which the REST server could build from the nodes directly. Arguments to
  keep them: one moment for all answers (a node asked bit by bit may see a
  change in between - not critical for display, as a view is stale when
  delivered and every operation checks its own permission), and plain
  data for tests and comparison (`Clone`, `PartialEq`) - which argues for
  a node handing out its loaded data as a value, not for a parallel
  hierarchy. So renaming the views to snapshots would be an intermediate
  step at most.
- **2026-09-30** — Decided by the maintainer: `SessionReader` goes up in
  the new views - it is not kept beside them, renamed or as a facade over
  them. What components read of their session today (`own_channel`,
  `readable_channels`, `notes_of`, `history_of`, `private_notes`,
  `channel_notes`, `privacy_of`) they read through the navigation, with the
  agent as viewer, starting in its channel; the component gets that view
  where it gets its `SessionReader` today (`ComponentContext`, issue 128).
