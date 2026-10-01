# 163 — There is no model to navigate from a session to what is in it

**Status:** done
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

## Plan (draft, 2026-09-30)

Drawn up for the work to begin on 2026-10-01. It sketches the interface,
the steps and what each step replaces. What it decides ahead is marked as
a proposal; the open decisions are listed at the end.

### Names

- **Views** are the navigation: `caelumora-runtime/src/view/` (`mod.rs`,
  `session.rs`, `channel.rs`, `member.rs`, `agent.rs`). A view is a node
  the viewer stands on; its methods are the edges, and each edge asks its
  own access question.
- Today's `caelumora_model::view` (`SessionView`, `ChannelView`,
  `MemberView`, `ParticipantView`) becomes `caelumora_model::snapshot`
  (`SessionSnapshot`, ...) in step 1, so that the name is free; whether it
  survives step 3 is decided there (History, 2026-09-30).
- `SessionReader` and its types (`PublishedNotes`, `ChannelInfo`,
  `ChannelHistory`, `ChannelNotes`, `ReadError`) go up in the views (steps
  4-5).

### The viewer

Who walks the graph, and from where. Two kinds of viewer read by two
different rules today, and the difference is real:

- A **user** reads a channel if they may read its document and are
  cleared for it (`may_read_channel` and `may_see_channel` with their
  roles, issues 70, 133) - all of it, as far as their history limit
  reaches (issue 46).
- An **agent** reads *from its channel*: another channel as a whole if
  its channel may read it (`may_channel_read(reader, source)`, issue
  133), else entry by entry - only what was released to it; and a channel
  it may not read at all is one that does not exist (no probing, issue
  17).

```rust
/// Who reads, and from where.
pub struct Viewer {
    /// In whose name every question is asked (issue 131).
    context: SecurityContext,
    /// For an agent: the place it reads from - its channel, that channel's
    /// classification and its session's privacy. `None` for a user.
    from: Option<ReaderPlace>,   // owned counterpart of `security::Place`
}
```

Proposal: one `Viewer` type with an optional place, not two navigations.
The edges differ in the question they ask, not in shape. An agent's
`context` is its actor's; `from` is filled where its `SessionReader` is
built today (`ActorRuntime::session_reader`, `AgentActor::new`).

### The interface (sketch)

```rust
// Root: SessionManager::views(viewer) -> Views
impl Views {
    pub async fn sessions(&self) -> Result<Vec<SessionView>>;          // those the viewer may read
    pub async fn session(&self, id: SessionId) -> ViewResult<SessionView>;
    pub async fn agent(&self, id: AgentId) -> ViewResult<AgentView>;   // the agent itself, off any channel
}

// A session, running or stored (`Found`, as `SessionManager` finds it today).
impl SessionView {
    pub fn id(&self) -> SessionId;
    pub fn name(&self) -> &str;
    pub fn privacy(&self) -> SessionPrivacy;                           // issue 134
    pub fn participants(&self) -> &[Participant];
    pub async fn may_change(&self) -> bool;                            // UpdateSession
    pub async fn channels(&self) -> Vec<ChannelView>;                  // only what the viewer may read
    pub async fn channel(&self, id: ChannelId) -> ViewResult<ChannelView>;
    pub async fn channel_named(&self, name: &str) -> ViewResult<ChannelView>; // for agents' tools
}

// A channel; holds the running loop if there is one, the document either way.
impl ChannelView {
    pub fn id(&self) -> ChannelId;
    pub fn name(&self) -> &str;
    pub fn moderator(&self) -> &str;
    pub fn classification(&self) -> Classification;
    pub fn channel_ref(&self) -> ChannelRef;
    pub async fn may_change(&self) -> bool;                            // UpdateChannel
    pub async fn members(&self) -> Vec<MemberView>;
    pub async fn member(&self, actor: ActorId) -> ViewResult<MemberView>;
    pub async fn history(&self) -> ViewResult<VisibleHistory>;         // limits (46); per message for an agent
    pub async fn notes(&self) -> ViewResult<Vec<PublishedNotes>>;      // every agent's; per note for an agent
}

// A participant of the channel - a user, an agent, an approver.
impl MemberView {
    pub fn id(&self) -> ActorId;
    pub fn name(&self) -> Option<&str>;                                // None: no longer exists
    pub fn handle(&self) -> Option<&str>;                              // crate::mention
    pub async fn may_read_context(&self) -> bool;                      // ReadAgentContext; agents only
    pub async fn notes(&self) -> ViewResult<AgentNotes>;               // its published notes + digest
    pub async fn context(&self) -> ViewResult<AgentContextView>;       // requires ReadAgentContext
}

// What an agent saw in a channel: its `AgentChannelState`, from the store.
impl AgentContextView {
    pub fn transcript(&self) -> &Transcript;
    pub fn private_notes(&self) -> (Vec<Note>, Option<Digest>);       // per note for an agent reading another channel
    pub fn context_of(&self, message: MessageId) -> Result<ResolvedContext, ContextError>; // NotRecorded | Unavailable
    pub fn given_up_turns(&self) -> Vec<GivenUpTurn>;                  // issue 145
}

// The agent itself, across channels.
impl AgentView {
    pub async fn memory(&self) -> ViewResult<Vec<Memory>>;            // owner (ReadAgentContext); as it is
    pub async fn recalled_in(&self, channel: &ChannelView) -> ViewResult<Vec<Memory>>; // what may be recalled there (AgentMemory::recalled)
}

pub enum ViewError {
    /// Does not exist, or the viewer may not read it - one case, so that
    /// nothing can be probed (as `SessionError::ChannelNotFound` and
    /// `ReadError::NotReadable` are today).
    NotFound,
    /// Exists and is the viewer's to see that it exists, but not to read
    /// (a member's context).
    Forbidden,
    Failed(anyhow::Error),
}
```

Rules that hold throughout:

- A node is data the viewer was allowed to reach; it decides nothing. An
  operation - sending, approving, adding a participant - still checks its
  own permission and still goes through the running loop
  (`routes::open_channel` stays for operations; reads no longer use it).
- Every edge asks exactly one question, in the viewer's `context`. The
  questions are the `AccessQuestions` that exist; no new ones are
  expected except `ReadAgentContext` being asked for a *former* member
  (below).
- Where a node reads: the session and channel documents from the
  `EntityStore` (in the runtime's name, as `describe_session` does
  today), an agent's channel state and memory from the `StateStore`
  (as `AgentActor::readable_state` does today, running or not), the
  channel's history from the running loop when there is one - the loop
  holds the truth in memory and writes the store after - and from the
  store with the channel's `history_limits` applied otherwise. `Found`
  (running/stored) in `SessionManager` is the switch, already there.

### Steps

Each step compiles, passes and can be committed on its own.

1. **The nodes and the snapshots.** `view/` with `Viewer`, `Views`,
   `SessionView`, `ChannelView`, `MemberView` and the questions
   `may_change`, `may_read_context`; `SessionManager::views(viewer)`.
   `caelumora_model::view` renamed to `snapshot`; `describe_session` and
   `describe_channel` become `SessionView::snapshot()` and
   `ChannelView::snapshot()`, so that `session_view(s)`/`channel_view`
   are thin and the REST server changes nothing yet. The tests of
   `session.rs` on views (`a_view_shows_what_its_viewer_may_read_and_do…`,
   `a_channel_its_viewer_may_not_read…`, `…handles…`) move to `view/`.
2. **Reads on the nodes.** `ChannelView::history` and `notes`,
   `MemberView::notes` and `context`, `AgentContextView`, `AgentView`.
   Moves: `AgentActor::readable_state`/`context_of` → `AgentContextView`;
   `ChannelLoop::published_notes`/`private_notes` → views;
   `AgentMemory::recalled`/`may_recall` → `AgentView::recalled_in`
   (its `privacy_of` lookup becomes internal to that edge). Removed from
   the `Actor` trait: `message_context`, `given_up_turns`, and with them
   `MessageContextError`/`ReadStateError` (→ `ViewError`, `ContextError`).
   Tests: with `MemoryStore`, stored and running.
3. **REST reads through the views.** Handler by handler:

   | Route | Today | Then |
   |---|---|---|
   | `GET /sessions` | `session_views` | `views.sessions()` |
   | `GET /sessions/{id}` | `session_view` | `views.session(id)` |
   | `GET …/channels`, `…/channels/{cid}` | `session_view`, `channel_view` | `session.channels()`, `session.channel(cid)` |
   | `GET …/messages` | `open_channel` → `visible_history` (starts the session) | `channel.history()` |
   | `GET …/notes` | `open_channel` → `published_notes` | `channel.notes()` |
   | `GET …/actors/{aid}/messages/{mid}/context` | `open_channel` → actor → `message_context` | `channel.member(aid).context().context_of(mid)` |
   | `GET …/actors/{aid}/given-up-turns` | likewise | `….context().given_up_turns()` |
   | channel snapshot of the event stream | `channel_view` + loop | `channel.snapshot()`, `history()`, `notes()` (the subscription stays on the loop) |

   `ApiError::from_runtime` maps `ViewError` (`NotFound` → 404,
   `Forbidden` → 403 `<subject>_forbidden`) and `ContextError`. The
   response types in `models.rs` are built from the nodes; if after this
   nothing but REST uses the snapshots, `caelumora_model::snapshot` goes
   (see History). The web UI needs no change unless step 6 changes what a
   route answers; verified in the browser.
4. **The agent as viewer.** The `from` place and the edges it changes:
   `SessionView::channels()` lists what the agent's channel may read as a
   whole; `channel_named` applies the no-probing rule (a channel not
   readable as a whole and with nothing released is `NotFound`);
   `history()` and `notes()` filter per entry by `may_channel_read`;
   `MemberView::context().private_notes()` likewise. `ComponentContext`
   gets `views: Views` and `channel: Option<ChannelView>` (the agent's
   own; `None` for the commands of a session) in place of
   `session_reader`. Components move one by one, each with its tests:

   | `SessionReader` today | Component | Then |
   |---|---|---|
   | `own_channel`, `own_privacy` | notes, memory | `context.channel`, `session.privacy()` |
   | `channel_notes` | notes | `channel.notes()` |
   | `readable_channels` | notes | `session.channels()` |
   | `notes_of(name)`, `history_of(name)` | notes, channel_history | `session.channel_named(name)?.notes()` / `.history()` |
   | `private_notes(agent)` | private_notes | `channel.member(me)?.context()?.private_notes()` per channel |
   | `privacy_of(session)` | agent_memory | inside `AgentView::recalled_in` |

   The tests that fake a `SessionReader` (`Privacies` in the memory
   component) become tests over a `MemoryStore` with two sessions.
5. **`SessionReader` goes.** `session_reader.rs`, `detached()`,
   `ActorRuntime::session_reader`, and the tests of `session.rs` that
   test the reader (`a_channel_reads_another_by_their_classification…`,
   `a_reader_knows_the_privacy…`, `a_released_message_is_read…`) become
   tests of the agent viewer in `view/`.
6. **Edges that need a decision** (each small, after the above):
   - *Former members.* An agent removed from a channel: its state is in
     the store, its messages in the history. Proposal:
     `ChannelView::member(aid)` finds it by the history it left, marked
     as former, so that its context stays readable; `members()` lists
     the current ones only.
   - *Edges into nothing.* `Memory::origin.channel` of a deleted channel,
     a `KnowledgeRef` into one: `Views::channel_of(ChannelRef) ->
     Option<ChannelView>`.
   - *Points in time.* `AgentContextView::at(digest)` and
     `ChannelView::history_at(digest)` over the chains' `prefix`, for
     "as the agent saw them when it produced message Z". Not needed by
     any caller yet; only if 08 or the UI asks for it.
7. **Documentation.** A section *Views* in CHANNELS.md (or a VIEWS.md of
   its own, listed in AGENTS.md), and the changes in ARCHITECTURE.md,
   REST-SERVER.md, MEMORY.md (reading other channels), COMPONENTS.md
   (`ComponentContext`), SECURITY.md (the views are where a frontend's
   access questions are asked).

### Open decisions, to take before step 1

- One `Viewer` with an optional place, or a user navigation and an agent
  navigation as two types (proposal: one).
- Whether the running loop or the store is the source of a channel's
  history while the session runs (proposal: the loop, through `Found`).
- Whether `Views` hangs on `SessionManager` or is built from the
  `RuntimeManager` directly (proposal: `SessionManager::views`, since it
  owns `Found`).
- Whether `caelumora_model::snapshot` is kept after step 3.
- Where the agent's reading classification comes from: its channel's now
  (`SessionReader`) or the turn's snapshot (`AgentMemory::recalled`) -
  the views see no turn, so the channel's now, unless the caller passes
  one.

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
- **2026-09-30** — A plan added (the section *Plan*): the viewer, the
  interface of the views, seven steps from the nodes over the REST reads
  and the agent as viewer to the removal of `SessionReader`, and the
  decisions still open. To be taken up on 2026-10-01.
- **2026-10-01** — The open decisions taken as proposed: one `Viewer`, the
  running loop as the source while a session runs (`Found`), the root on
  `SessionManager`, the snapshots renamed now and decided on in step 3,
  and the agent's reading classification set by whoever builds its viewer
  - which means the agent's views are built per turn, through the `Turn`,
  not once in the `ComponentContext` (to be laid out in step 4).
- **2026-10-01** — Step 1 done: `caelumora_runtime::view` with `Viewer`,
  `Views`, `SessionView`, `ChannelView`, `MemberView`, `ViewError`, and
  `SessionManager::views(viewer)` as the root; `caelumora_model::view` is
  `snapshot` (`SessionSnapshot`, `ChannelSnapshot`, `MemberSnapshot`,
  `ParticipantSnapshot`), taken by `snapshot()` on each node.
  `session_views`, `session_view`, `channel_view`, `describe_session`,
  `describe_channel` and `look_up` are gone from `SessionManager`.
  Deviation from the plan: the REST handlers that answered with views
  (`GET /sessions`, `/sessions/{id}`, `…/channels`, `…/channels/{cid}`,
  the channel and global snapshots of the streams) already read through
  the views - thin wrappers for one step were not worth keeping - so step
  3 is left with the reads of history, notes, contexts and given-up turns.
  `ViewError` is mapped by `ApiError::from_runtime` to the codes the
  routes answered before (`session_not_found`, `channel_not_found`,
  `actor_not_found`). `Views` holds an `Arc<SessionManager>`; an agent's
  root (step 4) is its own `SessionLoop`, for which the root will become
  a small trait over "where the running sessions are". The tests on views
  moved from `session.rs` to `view/`.
- **2026-10-01** — Steps 2 and 3 done together: the reads on the nodes
  and the REST handlers that use them went in one, since the `Actor`
  trait's reads could only go once nothing called them.
  - `ChannelView::history()` (the running loop's `visible_history`, or
    the stored `ChannelState` with the channel's `history_limits`) and
    `notes()`; `MemberView::notes()` and `context()`, which asks
    `ReadAgentContext` and answers an `AgentContextView` with
    `transcript()`, `private_notes()`, `context_of(message)` and
    `given_up_turns()`. `ViewError` gained `Forbidden` and `NoContext`;
    `ContextError` (`NotRecorded`, `Unavailable`) is the context's own.
  - Gone: `Actor::message_context` and `given_up_turns`, with
    `MessageContextError` and `ReadStateError`; `AgentActor::readable_state`
    and `context_of` (now `AgentContextView::context_of`, test moved).
  - REST: `GET …/messages`, `…/notes`, `…/actors/{aid}/messages/{mid}/context`
    and `…/given-up-turns` read through `routes::channel_of` and no longer
    start the session; `open_channel` is left to the operations and the
    event stream. `KnowledgeRefResponse::describe` names a channel through
    the views now, so a channel the viewer may not read is named by id only
    (before, every channel of the session was named).
  - Not moved yet: `ChannelLoop::published_notes` and `private_notes`,
    which `SessionReader` still uses (step 5); `AgentView` and the memory,
    which only the memory component needs (step 4).
  - A member is still one of the channel's participants: a removed agent's
    context stays unreachable until step 6.
- **2026-10-01** — Step 4, first half: the agent as viewer.
  - `Viewer::from_place(context, ReaderPlace)`: the agent's channel, the
    classification it reads at and its session's privacy. For such a
    viewer `SessionView::channels()` lists what its channel may read as a
    whole (`may_channel_read`), `all_channels()` and `channel_named()`
    reach the rest entry by entry (`Readable::Released`): `history()`,
    `notes()` and the new `MemberView::private_notes()` filter by each
    entry's classification, and answer `NoChannelNamed` when nothing
    could be read; `context()` needs the channel as a whole. A user
    viewer is unchanged. `AccessQuestions::may_read_session` was added
    for the root.
  - The agent's root is its own session, built once per turn:
    `ViewsFactory` (from `ActorRuntime::views`) resolves the channel and
    session loops when asked, `Turn::with_views` carries it and
    `Turn::session()` answers the `SessionView` at the turn's
    classification (decision 5). `Views` holds the runtime and a
    `RunningSessions` (the `SessionManager`, or the agent's one loop).
  - `notes`, `private_notes` and `channel_history` read through
    `turn.session()`; `PublishedNotes` is the views' type. `ReadError`'s
    role is `From<ViewError> for ToolError`.
  - Still on `SessionReader`: the memory (`own_privacy`, `privacy_of`),
    and so `ComponentContext::session_reader`. Second half next.
- **2026-10-01** — Step 4, second half, and step 5: the memory reads
  through the turn's views, and `SessionReader` is gone.
  - `AgentMemory::recalled`, `remember`, `forget` and `reclassify` take no
    reader: the session's privacy is `turn.session().privacy()`, that of
    the sessions entries come from `Views::privacy_of` - `pub(crate)`,
    looked up in the runtime's name, deliberately not an edge the agent
    could follow. A turn in no session recalls nothing and remembers as
    private (new test).
  - `ComponentContext` lost `session_reader`; `ActorRuntime::session_reader`
    and `session_reader.rs` with `detached()`, `ChannelSessionReader`,
    `ChannelInfo`, `ChannelHistory`, `ChannelNotes` and `ReadError` are
    deleted, and so are `ChannelLoop::published_notes` and
    `private_notes`, which only the reader used.
  - `ViewsFactory` reaches the session through the actor's runtime or,
    in tests, through a session loop held directly (`in_session`,
    `cfg(test)`); `ViewsFactory::session(channel, classification)` takes
    the turn's channel, so that place and turn agree.
  - Tests: the memory component's tests run over real sessions - stored,
    and running without channels - instead of a faked privacy lookup
    (`Privacies`); the case "a session that cannot be looked up" is not
    among them any more, since a store that fails cannot be faked there.
    The two reader tests of `session.rs` read through the views.
  - Open from the plan: step 6 (former members, edges into nothing,
    points in time) and the rest of step 7.
- **2026-10-01** — Verified in the browser, with `gemma4:26b` and a
  seeded agent that notes what it is told, in a session of two channels:
  the channel snapshot (members, handles), the history over
  `GET …/messages`, the notes panel, the context panel of an agent's
  message (transcript, tools, system prompt), and the agent reading the
  other channel's notes through `read_notes` without a channel - the
  agent viewer's `channels()` listed both. The knowledge read is named
  with its channels (`#main`, `#side`) through the views. A turn that
  failed (the model was still loading) is shown as "could not answer";
  given-up turns have no view in the UI yet (146). No change to the web
  UI was needed.
- **2026-10-01** — Step 6, decided by the maintainer:
  - *Former members:* `ChannelView::member(actor)` reaches an agent that
    left by the messages it left in the history the viewer sees
    (`MemberView::former`); `members()` lists the current ones. Its
    context stays readable to whom it is readable.
  - *Edges into nothing:* `Views::channel_of(ChannelRef)` answers `None`
    for a session or channel that is gone or not the viewer's to read.
    `KnowledgeRefResponse::describe` names channels through it - of
    another session too, if the viewer may read it. That the store should
    keep what was deleted and mark who left, so that gone and never there
    can be told apart, is [164](164-soft-deletion-and-former-members.md).
  - *Points in time* (`AgentContextView::at(digest)`,
    `ChannelView::history_at(digest)`): not built. No caller needs them;
    the chains answer a prefix by digest already. Open here and noted in
    [08](08-retry-and-edit-past-answers.md), which is where a caller would
    come from.
- **2026-10-01** — Step 7 and done. The views have their section in
  SECURITY.md (*Views*), since what they are about is who may reach what;
  ARCHITECTURE.md gained *Reading what is there*, which names the concept
  and points there and to MEMORY.md for the agent as viewer. No document
  of its own and no section in CHANNELS.md: SECURITY.md is where AGENTS.md
  sends whoever works on the views. "Done when" is met: the REST server's
  reads and what `SessionReader` did go through one navigation, with the
  permission asked at each step, for running and stopped sessions alike.
  Left open: points in time (noted in 08); gone and never-there told apart,
  and former members from the channel's document (164). The snapshots of
  `caelumora_model::snapshot` stay: the REST server builds its responses
  from them, and they are the nodes' data at one moment, not a parallel
  navigation.
- **2026-10-01** — The maintainer confirmed that the snapshots stay.
