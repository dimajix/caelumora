# 518 — An agent's memory is reached through its state, not as a system service

**Status:** open
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
  to be revisited once a second service stages (below, *Open questions*).

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

1. **What lies behind an agent.** `AgentMember` and `KernelBuilder::agent_member`; the kernel's
   own state behind an agent removed through it, as `AgentManager::remove_behind` does today. No
   behaviour changes. Files: `caelumora-kernel/src/{agent/manager,kernel/builder,scoped}.rs`.
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

### Open questions

1. **One document per agent** holding the chain, recommended, or a document per entry already, the
   direction of issue 166.
2. **The rights of the document:** attributes of its own, copied from the agent - which go stale
   when the agent's owner changes -, or `Within = Agent`, judged by the agent's attributes.
   Recommended: the second, to be checked against the `CheckedStore` in step 3; it decides what
   context `MemoryManager::of_agent` reads in.
3. **Two ways to stage:** the notes in their fields, the services in `StagedChanges`. Accepted for
   now; the notes may follow once a second service stages.
4. **Private knowledge.** `Turn::record_knowledge` tells by `KnowledgeSource::Memory` that the turn
   has seen private knowledge (`turn/mod.rs`, the `PrivateNotes | Memory` arm); with `Entity`, it
   cannot. Either `KnowledgeRef` carries `private`, set by whoever records the reading - so that
   the rule no longer hangs on variants the turn has to know, recommended -, or `Turn` gets
   `record_private_knowledge`.
5. **`AgentMember`** as a trait of its own, recommended, or `ScopeMember::remove_behind`
   generalised into a `Behind<E>` for scopes and agents alike: decided in step 1.

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
