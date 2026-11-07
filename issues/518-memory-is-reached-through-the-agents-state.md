# 518 — An agent's memory is reached through its state, not as a system service

**Status:** done
**Type:** refactoring
**Priority:** medium
**Area:** Context and knowledge
**Summary:** An agent's memory is part of its state, which the agent side is handed whole and changes through three calls of its `ActorRuntime`, and the agent side itself decides which entries may be recalled where, while the workspace, skills and artifacts are system services reached through a reader and a writer.

## The problem

Issue [516](516-the-runtime-trusts-the-agents-linked-into-it.md) made the kernel a modular one:
its own services are sessions, channels, actors and their state, and persistence; the workspace,
skills and artifacts are **system services** that extend it - a manager that decides and keeps,
views for the frontends, and for an agent's work a reader and a writer the kernel binds to every
piece of work (`Situation::reader`, `Turn::writer`, the pattern of *Step 5b* there). Memory was
named among them (decision 10 there), but it is not one yet:

- The memory lies in the agent's state (`AgentState::memory`), which the kernel keeps in an
  `AgentStateHandle` and the agent side reads whole (`ActorRuntime::agent_state`).
- The agent side changes it through three calls that are the memory's alone:
  `reserve_memory_id`, and `apply_agent_state(changes, after)` when a turn ends.
- Which entries may be recalled in a channel the agent side decides itself: its memory
  (`caelumora-agents/src/memory.rs`) asks the kernel's flow question for every entry
  (`ActorRuntime::may_copy_information`), and the component `agent_memory` writes the prompt part
  of what it found.

ARCHITECTURE.md says so: "memory is still reached through the agent's state".

## Why it matters

What an agent may recall where is a decision of the kernel's, as what a work may read of another
channel is (issue [131](131-agent-memory-bypasses-policy.md) made memory ask the policy at all).
Asked per entry by the agent side, it is kept by the agent side's loop: a library that leaves the
question out recalls what it may not. As a system service, the kernel hands the work only the
entries it may recall, as `SkillReader::usable_skills` hands only the skills a work may load.

It also slims the interface: `ActorRuntime` loses `reserve_memory_id` and `apply_agent_state`,
which serve one kind of state, and the agent's state the agent side is handed no longer carries
what it may not see in a channel.

## Considerations

- **The pattern of 516.** A `MemoryReader` - the entries the work may recall where it reads from -
  handed out by the `Situation`, and a `MemoryWriter` - remembering and forgetting, staged in the
  turn and applied when it ends - handed out by the `Turn`, both bound by the kernel to every piece
  of work; a manager of memory that decides and keeps, asking the `ConfidentialityPolicy`; views
  for the frontends (the web UI shows an agent's memory, issue
  [303](303-note-and-memory-presentations.md)).
- **The kernel's, or a plugin's.** Memory is part of an agent's state, which the kernel keeps
  (`AgentStates`); the service can be the kernel's own extension, as the workspace's is, or a
  plugin's, as skills are. The kernel's is the smaller step, a plugin the cut of 516 at its
  cleanest.
- **Staging.** A turn stages its changes and applies them when it completes (ARCHITECTURE.md,
  *Stage, then commit*); the writer keeps that: what it is told is applied at the turn's end, in
  the turn's name, or not at all.
- **Topics.** Issue [166](166-memory-as-topic-documents.md) wants memory as documents by topic,
  fetched as they are needed. The reader and the writer are the interface that change would be made
  behind; doing this first keeps 166 within the kernel's service.
- **Where a change is made is told, not derived.** Found while reading for the plan: the origin
  of every change - the channel, and the privacy its session had - is built by the agent side
  (`AgentMemory::origin`) and handed to `apply_agent_state` as data. A library could say an entry
  was written in a public session; nothing checks it. Whoever applies the changes has to derive the
  origin from the place of the work itself.

## The plan

Decided with the maintainer on 2026-11-04: memory becomes a **plugin in a crate of its own**,
`caelumora-memory`, standing on both levels as `caelumora-skills` does, with its security on the
kernel's side; the turn goes on staging its changes, which the kernel commits when the turn
completes (*plan A* of the two weighed below).

### What the plan decides

- **A plugin, not the kernel's extension.** The smaller step - the service in the kernel, the
  entries staying in `AgentState` - would leave the kernel keeping a kind it does not decide
  about. The entries leave the agent's state: the memory of an agent is a document of the plugin's
  kind, one per agent, holding the chain as it is today, so that the rules, the numbers and the
  digest stay as they are. Splitting it into documents by topic stays with issue 166, which the
  reader and the writer now hide. With the entries, `AgentState`, `AgentStateChange`,
  `AgentStates`, `AgentStateHandle`, `get_agent_state` and `put_agent_state` of the three
  backends and `agent::STATE_CHANGED` leave the kernel; the memory's types - `Memory`,
  `MemoryId`, `MemoryChange`, `MemoryOrigin`, the chain - leave the domain for the crate's layer
  without features. What it costs: a commit writes the whole document where it wrote the changes
  before, and across processes only the document's revision guards against two writers; within
  the process the manager's lock does.
- **Its security stays the kernel's.** Weighed: the memory is the agent's alone, private from
  every other agent and user, so the agent side might be made responsible for it - a new concept,
  a library trusted for one thing. Rejected: the flow rule does not protect the agent but the
  session the agent heard something in. An entry written in a private session must not reach the
  model's context in a public one, since the context flows into what the agent says there; the
  owner of the information is the place it was heard, not the agent. That is the same decision as
  `GetChannel` and `may_copy_information` for another channel, and lies where they lie. The owner
  reads the memory through the views as well. What stays the agent side's is the turn's view -
  the entries recalled, with the turn's own changes in them - and, as for everything it says, the
  classification of a new entry (the turn's label); the kernel checks only that it does not exceed
  the channel.
- **The turn stages, the kernel commits** (*plan A*). The turn keeps staging its changes, as it
  stages its notes, and hands them to the kernel when it completes; the writer bound to the work
  derives the origin from the work's place and checks every change: an entry changed is one
  recalled here or added in the same turn, a classification within the work's, a reclassification
  permitted. There is one place a turn stages, the changes are kept with the channel state and
  survive a restart, and a turn given up leaves none. What it costs: a tool asks `may_reclassify`
  once so that it can answer the model, and the kernel asks again as it commits; a change the
  kernel refuses at the commit is lost and logged; the origin carries the session's privacy at the
  commit, not at the call. The alternative, *plan B* - the writer stages in the kernel, per work,
  applied or dropped by generic calls of `ActorRuntime` when the work ends -, would leave the agent
  side with almost nothing, but make a second place that stages, bind the faces to a turn, and
  make the kernel keep what a turn staged across a restart.
- **The staging is generic.** `TurnState::memory` cannot stay: the domain will not know its type.
  The turn gets one place for what it stages for system services, by a kind each
  (`StagedChanges`), and the kernel hands each kind to the writer bound to commit it. Notes and
  private notes keep their fields of `TurnState`: they change the agent's state in its channel,
  which the agent side holds and stores itself, not a system service - a deliberate difference,
  to be revisited once a second service stages (below, *Decisions on the open questions*).

### Where it lives

| Part | Side | Crate, feature |
|---|---|---|
| `Memory`, `MemoryId`, `MemoryChange`, `MemoryOrigin`, the chain, the entity `AgentMemory` | both read it | `caelumora-memory`, no feature |
| `MemoryReader`, `MemoryWriter`, `MemoryError` | the contract | `caelumora-memory`, `api` |
| `MemoryManager`, `MemoryAccess`, the reader's and the writer's implementations, `MemoryPlugin` | kernel | `caelumora-memory`, `kernel` |
| the component `agent_memory`, `MemoryLibrary`, its texts | agent | `caelumora-memory`, `library` |
| `StagedChange`, `Committing`, `ActorRuntime::commit_staged` | the generic contract | `caelumora-api` |
| `TurnState::staged`, `StagedChanges` | domain | `caelumora-domain` |
| `Turn::stage`, `Turn::staged` | agent | `caelumora-agents` |
| kind to writer, the commit, what lies behind an agent | kernel | `caelumora-kernel` |

No views for the frontends while none reads the memory (issue 303): they come as a `MemoryViews`
on `Views` when one does.

### The interfaces

The contract of the memory, `caelumora-memory` under `api`:

```rust
/// What of its agent's memory a work may recall where it reads from: a snapshot, taken when
/// the reader was made.
#[async_trait]
pub trait MemoryReader: Send + Sync {
    /// The entries the work may recall where it reads from, as the memory was then.
    async fn recalled(&self) -> Vec<Memory>;
    /// The digest of the memory as a whole then: what a work that recalls from it records.
    fn digest(&self) -> Option<Digest>;
}

/// What a turn changes of its agent's memory; the changes themselves it stages
/// (`StagedChange`) and the kernel commits when the turn completes.
#[async_trait]
pub trait MemoryWriter: Committing<MemoryChange> {
    /// A number for a new entry, never handed out before: reserved at once, since the agent's
    /// turns elsewhere remember at the same time.
    fn reserve_id(&self) -> MemoryId;
    /// Whether an entry may be reclassified as `classification` here: what a tool answers
    /// before the turn stages it; asked again when it is committed.
    async fn may_reclassify(&self, classification: &Classification) -> bool;
}

/// A change to the memory, as a turn stages it: without where it was made, which the kernel
/// knows better than the turn.
pub enum MemoryChange {
    Add { id: MemoryId, text: String, classification: Classification },
    Update { id: MemoryId, text: String, classification: Classification },
    Remove { id: MemoryId },
    Reclassify { id: MemoryId, classification: Classification },
}
impl StagedChange for MemoryChange { const KIND: &'static str = "memory"; }
```

The snapshot is the reader itself: the binding loads the document, the reader holds it, so that
`recalled` and `digest` read one state, as both read `agent_state()` today, and "asked for, never
kept" gives every call a fresh one. `recalled` is asynchronous since the privacy of other sessions
is looked up; an entry whose session cannot be looked up is not recalled and logged, as today.

The contract of the staging, `caelumora-api`:

```rust
/// A change a turn stages for a system service, committed when the turn completes.
pub trait StagedChange: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The name its changes are kept and committed under.
    const KIND: &'static str;
}

/// A writer that is handed what a turn staged for it when the turn completes.
#[async_trait]
pub trait Committing<C: StagedChange>: Send + Sync {
    /// Applies `changes`, in the order they were staged, of a turn that ended with message
    /// `after`, in the work's name, each checked.
    async fn commit(&self, changes: Vec<C>, after: MessageId) -> CommitResult<()>;
}

// ActorRuntime, in the place of apply_agent_state:
/// Hands what the turn staged to the writers bound to commit each kind
/// (`KernelBuilder::commits`), in the work's name. A kind no writer is bound for is logged and
/// dropped.
async fn commit_staged(
    &self,
    initiator: ActorId,
    classification: Classification,
    staged: StagedChanges,
    after: MessageId,
) -> CommitResult<()>;
```

`CommitError` is the kernel's type (`Refused`, `Store`, `Invalid { kind, why }`), in which every
plugin answers, as PLUGINS.md asks.

The domain and the turn:

```rust
// caelumora-domain, in TurnState in the place of `memory`:
/// What the turn staged for system services, by kind, each a list of changes in the order
/// staged; kept with the channel state, so that a turn survives a restart with it.
pub struct StagedChanges(BTreeMap<String, Vec<Value>>);
impl StagedChanges {
    pub fn push<C: StagedChange>(&mut self, change: C);
    pub fn of<C: StagedChange>(&self) -> Vec<C>;
}

// caelumora-agents, Turn:
/// Stages `change` for its service, committed when the turn completes.
pub fn stage<C: StagedChange>(&mut self, change: C);
/// What the turn has staged for `C` so far: what its own view is built from.
pub fn staged<C: StagedChange>(&self) -> Vec<C>;
```

The kernel's side, `caelumora-kernel`:

```rust
// KernelBuilder, beside extend_writers:
/// Lets the writer `T`, bound by `extend_writers`, commit what a turn stages as `C`.
pub fn commits<C: StagedChange, T: ?Sized + Committing<C> + 'static>(&mut self) -> Result<()>;
/// Makes `member` go with an agent: removed behind it when the agent is purged.
pub fn agent_member<M: AgentMember + 'static>(&mut self, member: Arc<M>);

/// What a plugin keeps behind an agent, as `ScopeMember::remove_behind` keeps behind a scope.
#[async_trait]
pub trait AgentMember: Send + Sync {
    async fn remove_behind(&self, agent: &Agent) -> StoreResult<()>;
}
```

Two calls, `extend_writers` and `commits`, rather than one that does both: the second is only the
pairing of a kind with a type, and `extend_writers` stays as skills and artifacts use it.

The kernel's side of the plugin, `caelumora-memory` under `kernel`:

```rust
/// The memory's manager: decides and keeps, asking the policies.
pub struct MemoryManager { … }
impl MemoryManager {
    /// The memory of `agent` as `context` reads it.
    pub async fn of_agent(&self, context: &SecurityContext, agent: AgentId)
        -> MemoryResult<AgentMemory>;
    /// Of `memory`, what may be recalled at `here` in `context`: the flow rule once per
    /// channel, classification and session the entries come with, the privacy of the other
    /// sessions looked up.
    pub async fn recallable(&self, context: &SecurityContext, here: &Place, memory: &AgentMemory)
        -> Vec<Memory>;
    /// A number for a new entry of `agent`, never handed out before.
    pub fn reserve_id(&self, agent: AgentId) -> MemoryId;
    /// Applies `changes` made at `here`, each checked - an entry changed is one recalled at
    /// `here` or added in `changes`, a classification within `within`, a reclassification
    /// permitted -, under the agent's lock; stores and announces.
    pub async fn apply(&self, context: &SecurityContext, here: &Place, within: &Classification,
        changes: Vec<MemoryChange>, after: MessageId) -> MemoryResult<()>;
}

/// The plugin's question of the AccessPolicy, as SkillAccess asks its own.
pub trait MemoryAccess {
    async fn may_reclassify_memory(&self, context: &SecurityContext, author: ActorId,
        attributes: &SecurityAttributes, classification: &Classification) -> bool;
}
```

The binding makes the reader and the writer from the `SessionView` of the work: the agent from its
viewer, the place from its channel and its session's privacy, the work's classification as
`within`. From these the kernel derives the `MemoryOrigin` as it commits; the agent side no longer
supplies it.

The agent's side, `caelumora-memory` under `library`: the component `agent_memory` moves out of
`caelumora-agents` with its tools and its prompt part. What is left of `AgentMemory` is the turn's
view, without a question of the policy:

```rust
/// The memory as the turn sees it: what may be recalled here, with the turn's own changes in it.
pub(crate) struct TurnMemory { recalled: Vec<Memory>, staged: Vec<MemoryChange> }
impl TurnMemory {
    /// From the reader and `Turn::staged`; `None` where no reader is bound.
    pub(crate) async fn of(turn: &Turn<'_>) -> Option<Self>;
    /// The entries recalled, with the staged changes applied.
    pub(crate) fn entries(&self) -> Vec<Memory>;
    /// Entry `id`, recalled here or added in this turn.
    pub(crate) fn find(&self, id: MemoryId) -> Option<Memory>;
}
```

`remember` becomes `writer.reserve_id()` and `turn.stage(MemoryChange::Add { …, classification:
situation.label() })`; `update` and `forget` check `find` only so that the tool can answer the
model at once - the kernel checks again as it commits. The prompt part records its reading as
`KnowledgeSource::Entity { kind: memory, id: agent, digest }`, as skills record theirs.

### What goes

- From `ActorRuntime` and `Situation`: `agent_state`, `reserve_memory_id`, `apply_agent_state`,
  `may_copy_information` - used by the memory alone, the private notes ask through the views.
- `OwnRecord::Memory` and `Reclassified::Memory`: the plugin asks its own question
  (`MemoryAccess`); `OwnRecord` is left with `Note` alone and goes, `may_reclassify` taking the
  note's target.
- `KnowledgeSource::Memory`, for `Entity` with the memory's kind: the REST answer goes on saying
  `source: "memory"`, so that the web UI stays compatible.
- `AgentState` and all that keeps it (above).

### The steps

1. **What lies behind an agent.** `AgentMember` and `KernelBuilder::agent_member`, removed by
   `AgentManager::remove_behind` after the kernel's own state of the agent, which the manager
   removes itself (see History). No behaviour changes. Files:
   `caelumora-kernel/src/{agent/mod,agent/manager,kernel/builder}.rs`.
2. **The generic staging.** `StagedChange`, `Committing`, `CommitError`,
   `ActorRuntime::commit_staged`, `StagedChanges` in `TurnState`, `Turn::stage` and `staged`,
   `KernelBuilder::commits`. The memory uses it first, still from `caelumora-agents`, through a
   `MemoryWriter` the kernel binds to the `AgentStateHandle`; after it, `reserve_memory_id`,
   `apply_agent_state` and `may_copy_information` have left `ActorRuntime`. Files:
   `caelumora-api/src/runtime.rs`, `caelumora-domain/src/turn.rs`,
   `caelumora-agents/src/{turn/mod,turn/ending,memory}.rs`,
   `caelumora-kernel/src/kernel/{agent,builder}.rs`, `agent/state.rs`.
3. **The crate.** `caelumora-memory`: the entity, the manager, the plugin and the library; the
   memory's types move out of the domain; `AgentState` and its store methods go; `MemoryAccess`
   in the place of `Reclassified::Memory`; `KnowledgeSource::Memory` becomes `Entity`, with the
   replacement for private knowledge (below). The tests of recalling move from the security suite
   of `caelumora-agents` into the plugin's tests, in the world of `caelumora_kernel::testing`.
   Files: the new crate, `caelumora-domain/src/{memory,agent/state,context/item}.rs`,
   `caelumora-store/src/{lib,memory,suite}.rs` and the JSON and Postgres backends,
   `caelumora-agents/src/components/agent_memory.rs`, `caelumora-app`, `caelumora-tui`.
4. **The documents.** MEMORY.md (the service, the reader, the writer, the staging), ARCHITECTURE.md
   (memory among the system services without a reservation), PLUGINS.md (what lies behind an
   agent, the generic staging), STORAGE.md (no agent state), CRATES.md.

Stored data to delete: every agent's state - its memory with it, unless it is migrated on purpose -
and every channel state with a turn under way.

### Decisions on the open questions

Taken with the maintainer on 2026-11-07, as recommended:

1. **One document per agent**, holding the chain as `AgentState` holds it today. Splitting it into
   documents by topic stays with issue 166.
2. **The rights of the document are its own.** `Within` has no bearing on rights: the
   `CheckedStore` judges every kind by its own `security_attributes()` (`caelumora-store`,
   `access.rs`). The document is `owned_by` the agent's owner, without an access list, and keyed
   by the agent's id, lying in no entity (`Within = NoEntity`). An agent's work reads it in its
   context, whose owner is the agent's (`SecurityContext::of_agent`), and passes the ownership
   policy. Nothing in the kernel changes an agent's owner today, so the copy cannot go stale yet.
3. **Two ways to stage**, accepted: the notes in their fields of `TurnState`, the system services
   in `StagedChanges`. The notes may follow once a second service stages.
4. **Private knowledge is a field.** `KnowledgeRef` carries `private`, set by whoever records the
   reading - the private notes and the memory -, and `Turn::saw_private_knowledge` asks it; its
   one user is whether the reasoning is shown to the owners alone (`actor/agent.rs`). The rule no
   longer hangs on variants the turn has to know.
5. **`AgentMember` is a trait of its own**: `ScopeMember` asks for a store and events, which what
   lies behind an agent does not need.

Stored memories are deleted, not migrated: a migration would be code without a lasting user.

## Done when

- An agent's work reaches its memory through a reader and a writer the kernel binds, and the
  reader answers only the entries the work may recall where it reads from.
- `ActorRuntime` has no call of the memory's own (`reserve_memory_id`, `apply_agent_state`), and
  the agent side asks no flow question of an entry of its memory.
- The memory is a plugin's, `caelumora-memory`; `AgentState` is gone from the kernel and the
  stores, and where a change was made is derived by the kernel from the work's place.
- A turn stages what it changes of a system service in one generic place, committed by the
  kernel.
- ARCHITECTURE.md names memory among the system services without a reservation.

## History

- **2026-11-01** — Opened from issue [516](516-the-runtime-trusts-the-agents-linked-into-it.md),
  whose decision 10 names memory among the extensions of the kernel and whose steps left it reached
  through the agent's state.
- **2026-11-04** — Planned with the maintainer: a plugin in a crate of its own, its security the
  kernel's, the turn staging and the kernel committing, the staging generic (*The plan*). Found
  on the way: the origin of a change is told by the agent side, not derived. Not begun.
- **2026-11-07** — The open questions decided as recommended (*Decisions on the open questions*).
  Step 1 implemented: `AgentMember` and `KernelBuilder::agent_member`; the `AgentManager` is made
  once the plugins registered, and removes what they keep behind an agent after the agent's state,
  as it purges it alone or with its scope. Unlike the step's first wording, the kernel's own state
  is not removed through an `AgentMember` of the kernel's: the manager removes it itself, as the
  `WorkspaceManager` removes its place - an owner removes what is its own, a member is what others
  add -, a member of the kernel's would only wrap one call, and `delete_agent_state` removes the
  agent's states in its channels too, which stay with the kernel after step 3. So `agent_member`
  has no caller in the program until step 3 brings the memory's plugin.
- **2026-11-07** — Step 2 implemented: a turn stages what it changes of a system service in
  `TurnState::staged` (`StagedChanges`), and the kernel hands it, kind by kind, to the writer bound
  to commit that kind (`ActorRuntime::commit_staged`, `Committing`). The memory is the kernel's own
  extension until step 3 (`caelumora-kernel/src/agent/memory.rs`): its reader hands the work only
  what it may recall, its writer reserves numbers and commits a turn's changes, each checked, all or
  none, made where the work reads from. `agent_state`, `reserve_memory_id`, `apply_agent_state` and
  `may_copy_information` left `ActorRuntime`. Where it differs from *The interfaces* above:
  `StagedChange` lies in the domain beside `StagedChanges`, which names it, not in `caelumora-api`;
  `StagedChanges::push` and `of` and `Turn::stage` and `staged` can fail, since what is staged is
  kept as JSON with the turn, and `push` and `stage` borrow the change; `commit_staged` is handed
  the turn's `StagedChanges` and one kind a call, so that the turn logs each failure with its kind;
  `CommitError` is `NotOffered`, `Unreachable`, `Unreadable`, `Invalid` with words for the log,
  `Refused` with the policy's refusal, and `Store`; `KernelBuilder::commits` waits for step 3, its
  first caller outside the kernel, which registers its own through `Extensions`;
  `MemoryReader::recalled` is no longer asynchronous, the reader being the snapshot made as it is
  bound; `OwnRecord` went now, the kernel's writer asking `Reclassified::Memory`, and
  `ActorRuntime::may_reclassify` became `may_reclassify_note`; the turn's view stays `AgentMemory`,
  which asks whether the turn sees an entry (`knows`) rather than finding it. New in the kernel:
  `AgentStates::held`, by which the bindings find the handle an actor of the agent holds, and
  `AgentStateHandle::is_reserved`; the runtime of an agent holds the handle without reading it
  (`_state`), and `AgentRuntime::in_test` still takes the handle, its work reaching the memory only
  where it is the kernel's - making it fetch the handle itself would make it asynchronous for 25
  callers, for what step 3 removes. What changes in behaviour: a turn that reaches no session can no
  longer remember; the privacy recorded with a change is its session's at the commit, not at the
  call; a number given before - removed since, or twice in one turn - is refused. A number another
  turn of the agent reserved passes the check, since who reserved it is not kept. The checks run on
  a snapshot taken before the handle's lock, so that an entry another actor removes meanwhile is
  updated by nothing.
- **2026-11-07** — Step 3 implemented: `caelumora-memory`, a plugin and a library on the levels of
  `caelumora-skills` (`api`, `kernel`, `library`). The memory's types left the domain for the crate;
  an agent's memory is a document of the plugin's kind (`AgentMemory`, kind `memory`), one per agent
  under the agent's id, owned by the principal of the agent's work - its owner -, as everything made
  in someone's name is; the `MemoryManager` keeps the numbers and the lock per agent it serves, has
  a commit judged under the lock on the memory as it reads it there and applies it, and removes the
  memory with its agent (`AgentMember`, through the kernel's `purge_for_good`, now public); the
  reader and the writer (`view.rs`) decide what may be recalled and judge every change, the writer
  handing its judgement to `MemoryManager::apply`; reclassifying asks the plugin's own verb
  (`RECLASSIFY`, `MemoryAccess`) in the place of `Reclassified::Memory`. The numbers are begun from
  the memory read in the kernel's name, and raised to what a commit finds stored. `AgentState`,
  `AgentStates`, `AgentStateHandle`, `get_agent_state` and `put_agent_state` of the three backends,
  the table `agent_state` and `agent::STATE_CHANGED` are gone; `delete_agent_state` became
  `delete_agent_channel_states`, as the states in the channels are all that is left of it;
  `AgentRuntime` holds no state, and `in_test` takes none. `KnowledgeSource::Memory` became `Entity`
  of the kind `memory`, and `KnowledgeRef::private` says what is private knowledge. Where it differs
  from the plan above: `MemoryChange` is a type of the crate's own around `Change<MemoryId>`, since
  the crate may not implement the domain's `StagedChange` for the domain's `Change`;
  `MemoryWriter::reserve_id` is asynchronous and answers `StoreResult`, the manager reading the
  memory once to count on from it; there is no `MemoryError`, which nothing needed; the turn's view
  is `TurnMemory`, the name `AgentMemory` now the document's; the checks the plan gave the manager
  are the writer's, since the recall rule reads through the work's views, which a manager is never
  given - the manager runs them under its lock; the manager is the crate's own, offered as no
  service, which no view needs yet; `agent::STATE_CHANGED` is not announced again, nobody listened
  to it. What changes in behaviour: the memory is read and written in the agent's name as a document
  of its owner, each read and write asked of the policy; the knowledge of memory reaches the web UI
  without the agent's name, which shows it as "Its memory". New for tests: `Turn::take_over` in
  `caelumora_agents::testing`, and `session::testing::owned` in the kernel. Stored data to delete:
  the agents' states - `runtime/agent_state` of the JSON store -; every agent channel state with a
  turn under way; the histories, transcripts and channel states of channels where an agent with
  `agent_memory` recalled something, whose records name the removed `KnowledgeSource::Memory`; and a
  Postgres database, whose migration `0002_states.sql` changed.
- **2026-11-07** — Step 4: the documents describe the memory as the system service of
  `caelumora-memory` - MEMORY.md (the document, the reader and the writer, the keeper, private
  knowledge), ARCHITECTURE.md (the crate, memory among the plugins' system services without a
  reservation, no agent's own state in the `StateStore` nor a writer of it), PLUGINS.md (the third
  crate on both levels, `KernelBuilder::commits`, the staged changes), STORAGE.md (one keeper per
  process in the place of the agent state's owner, no `agent_state` table, what goes with a purged
  agent), EVENTS.md, SECURITY.md, CONFIDENTIALITY.md (who reclassifies an entry, where component
  state is judged), PERMISSION-CHECKS.md (`ReclassifyMemory`, the memory's reading and commit),
  CHANNELS.md, AGENT-TURN.md and COMPONENTS.md; the graph of CRATES.md was written in step 3. Every
  point of *Done when* holds: done.
- **2026-11-07** — After the step 4 review, as the maintainer decided: a commit to an agent's memory
  that another process outdated is read again, judged again and written once more
  (`MemoryManager::apply`), as every writer of a document does; a memory stored but unreadable in
  the work's context is a conflict rather than made again. Whether the memory announces its changes
  stays to be decided. `scripts/wrapping.sh` finds every line of prose that begins with a dash -
  `-,` and `--all` as much as `- ` -, the lines it then found rewrapped, in three resolved issues
  too, their text unchanged.
- **2026-11-07** — The memory announces what it stored, as the maintainer decided: the
  `MemoryManager` announces a memory made as `Created` and one changed as `Updated` without what
  changed, to whoever may read it, its owner - as the rule of ARCHITECTURE.md asks of every writer
  of a document, which a frontend and other instances rely on. Dropping `agent::STATE_CHANGED` in
  step 3 for want of a listener was wrong: an announcement is part of a writer's contract, not an
  option waiting for a user.
