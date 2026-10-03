# 211 — `Turn` is the agent's handle between turns, the components' API and the turn's logic at once

**Status:** in progress
**Area:** Code structure

## The problem

[206](206-one-agent-channel-state.md) gave both kinds of agent one `AgentChannelState`. What works
on that state was left as it had grown, and reads unevenly:

**`Turn` (`caelumora-runtime/src/agent_turn.rs`, 1310 lines, about 50 methods) is three things.**

1. *The agent's handle on its state in the channel, also between turns.* The `AgentActor`'s run
   loop builds a `Turn` in every iteration, in `Idle` too. Hence `NO_TURN` ("what a turn reads as
   when none runs"), the `debug_assert` in `current_mut`, `agent_channel_state_mut`,
   `set_processing_state`, and a `begin` for tests.
2. *The API components work with during a turn:* `label`, `see`, `record_knowledge`, `stage_memory`,
   `stage_note`, `set_plan`, `request_state`, `session`. About twenty of them pass on to
   `TurnState` in one line.
3. *The turn's logic for the actor:* its life cycle (`next_turn`, `commit_turn`, `give_up_turn`,
   `record_turn`, `abandon_tool_calls`), what the model is given (`full_context`, `set_context`,
   `keep_prompt`, `place_turn_prompt`, `keep_tools`), the plan's progress (`advance_plan`,
   `plan_converged`) and sending (`send_event`, `EndedTurn`).

**The two kinds of agent are handled differently where they do the same.**

- The `ExternalAgentActor` holds its `AgentChannelState` and works on it; the `AgentActor` reaches
  it through `Turn` only, also where no turn is concerned (perceiving, storing).
- `Answering` (`actor_impl/external_agent.rs`) and `EndedTurn` with the free `send`
  (`agent_turn.rs`) are the same thing - the two contexts something is published in, and the
  sending - written twice.
- `catch_up` is the same in both actors but for which messages are taken; the text told for an
  error nobody may see ("internal error - the details are in the server log") is in both.
- `TurnState` and `RemoteState` are not counterparts, though they sit side by side in the state:
  a `TurnState` is one piece of work under way with the contexts it acts in, a `RemoteState` is
  what is kept of a connection (`sent_up_to`, `conversation`) *and* a list of such pieces of work.
  The counterpart of a `TurnState` is the `OpenTask` - which keeps only the event context of its
  request (`requested`), not whose authority it acts with: `resume`, `cancel` and what it says
  later act as the agent itself, where `send` acted in the request's name.

**Rules of the state live in the runtime.** `record_turn` and `give_up_turn` keep what must hold
between transcript and history - the chain of digests continued, both heads moved back, the branch
marked as given up - in `Turn`, while `perceive`, `TurnState::push` and all of `RemoteState` are in
`caelumora-model`. `caelumora-model/src/agent.rs` (682 lines) holds `Personality`, `Agent`, both
states and the turn's working state.

## Why it matters

- A reader of `Turn` cannot tell what a component may call from what only the actor does, nor
  which of it is the state's own rule. Every issue on the turn (75, 83, 85, 136, 137, 145, 148,
  157, 168) added to the one type.
- [205](205-external-agent-message-context.md) gives an external agent's answers a recorded
  context. It needs the `OpenTask` to be what a `TurnState` is for an agent run here, and the two
  actors to publish and record alike.
- [212](212-external-task-during-reclassification.md) needs an open task to know what it began
  with.

## Decided

On 2026-10-03, with the maintainer, after reading the code together:

- **`Turn` exists only while a turn runs.** Between turns the `AgentActor` holds its state itself,
  as the `ExternalAgentActor` does.
- **The state's rules move to the model, in the small form:** `TurnState` gets a file of its own,
  and what keeps transcript and history consistent becomes methods of `AgentChannelState`. The
  orchestration, the tool calls (`ToolCalls`, issue 83) and every text told to a model stay in the
  runtime. The large form - all of a turn's logic in the model - was weighed and not chosen: `Turn`
  would hardly shrink, a turn's rules would be split over two crates (`ToolCalls` cannot move),
  and giving a turn up would become two phases with a message handed in.
- **The rest of `Turn` is split by subject into files,** as `impl Turn` blocks: no new types.
- **One `Answering` for both actors.** `EndedTurn` goes.
- **`OpenTask` is aligned with `TurnState`** and stays in `RemoteState`: its `TaskId` is the remote
  side's, and an agent run here has exactly one piece of work, its turn. It is moved up beside
  `turn` only if agents run here ever get work in the background.
- **No type is replaced by a tuple.** The candidates were looked at: `EndedTurn`/`Answering` are
  merged instead; `CommittedTurn` keeps three named parts; `GivenUpTurn` crosses into the REST
  server; `Planned`, `Single`, `Registration`, `Party`, `ChannelEntry` and `Interface` are values
  kept in collections, not return values.

## Plan

### The target

```
caelumora-model/src/
    agent.rs            Personality, Agent, AgentChannelState (+ begin_turn, record_turn,
                        give_up_turn), AgentState
    turn.rs             TurnState, TurnEntry and their rules               (new, moved)
    external_agent.rs   RemoteState, OpenTask (aligned with TurnState)

caelumora-runtime/src/
    actor.rs            Actor, Answering                                    (Answering new here)
    agent_turn/
        mod.rs          Turn: the handle and what components work with
        ending.rs       commit, give_up, CommittedTurn, what is told of a turn given up
        context.rs      full_context, set_context, place_turn_prompt
        plan.rs         advance_plan, plan_converged, the plan's instructions
    tool_calls.rs       ToolCalls                                           (unchanged)
    actor_impl/
        mod.rs          perceive, catch_up, failure_message, event_for      (shared by both)
        agent.rs        AgentActor: holds the state, opens a Turn for a turn's steps
        external_agent.rs  ExternalAgentActor: holds the state
```

Who is what, in a sentence each:

| Type | Where | What it is |
|---|---|---|
| `AgentChannelState` | model | An agent in one channel: what it heard, said and was given, and the rules that keep these consistent. |
| `TurnState` | model | The working state of the one turn of an agent run here: what it is, and what it produced and staged. |
| `RemoteState` | model | What an external agent keeps of the remote side: how far it was told, the conversation, the open tasks. |
| `OpenTask` | model | One piece of work the remote agent has not ended, with the contexts it is answered in. |
| `Turn` | runtime | The handle on a turn under way: what components read and stage, and the steps the actor drives. |
| `ToolCalls` | runtime | The bookkeeping of one step's tool calls. |
| `Answering` | runtime | The contexts something is published in, and the sending. |
| `CommittedTurn` | runtime | What is still needed of a turn once it is taken over. |

### The APIs, as expected

`caelumora-model/src/turn.rs` - moved unchanged:

```rust
pub struct TurnState { /* as today */ }
pub struct TurnEntry { pub node: Node<TranscriptEntry>, pub context: ContextRef }

impl TurnState {
    pub fn pending_approvals(&self) -> Vec<ApprovalRequest>;
    pub fn push(&mut self, message: Message);
    pub fn messages(&self) -> impl Iterator<Item = &Message>;
    pub fn record_knowledge(&mut self, reference: KnowledgeRef);
    pub fn label(&self) -> Classification;
    pub fn see(&mut self, classification: &Classification);
    pub fn see_classified(&mut self, classification: Option<&Classification>);
    pub fn see_message(&mut self, message: &Message);
}
```

`AgentChannelState` - `perceive` and `recorded_context_of` as today, new:

```rust
impl AgentChannelState {
    /// Begins a turn: fixes what it is, keeps its tool set, and has it see the transcript.
    pub fn begin_turn(&mut self, event_context: EventContext, security_context: SecurityContext,
        classification: Classification, tools: Vec<Tool>, profile: ModelProfile);

    /// Takes a turn's entries and note changes over; whether notes changed.
    pub fn record_turn(&mut self, entries: Vec<TurnEntry>, notes: Vec<NoteChange>,
        private_notes: Vec<NoteChange>) -> bool;

    /// Keeps a turn's entries as a branch no longer active, given up for `reason`.
    pub fn give_up_turn(&mut self, entries: Vec<TurnEntry>, reason: GivenUpReason);
}
```

`Answering` (`caelumora-runtime/src/actor.rs`):

```rust
pub(crate) struct Answering {
    pub(crate) event_context: EventContext,
    pub(crate) security_context: SecurityContext,
}

impl Answering {
    /// The contexts of `request`, continued in the name of the actor `id`, called `name`.
    pub(crate) fn to(request: &Envelope, id: ActorId, name: String) -> Self;
    /// Sends `event` as it is published; one with nothing left to show is not sent.
    pub(crate) async fn send(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()>;
    /// Sends `events` in turn; one that cannot be sent is logged.
    pub(crate) async fn send_all(&self, events: Vec<ActorEvent>, output: &dyn EventSender);
}
```

`Turn` (`agent_turn/`):

```rust
impl<'a> Turn<'a> {
    // mod.rs - the handle
    /// The turn under way in `state`; `None` between turns.
    pub fn of(agent_id: AgentId, agent_name: String, state: &'a mut AgentChannelState,
        agent_state: Arc<AgentStateHandle>) -> Option<Self>;
    pub fn with_views(self, views: ViewsFactory) -> Self;

    // mod.rs - what components work with, as today
    pub fn agent_id(&self) -> AgentId;
    pub fn agent_name(&self) -> &str;
    pub fn agent_state(&self) -> &Arc<AgentStateHandle>;
    pub fn agent_channel_state(&self) -> &AgentChannelState;
    pub fn session(&self) -> Option<SessionView>;
    pub fn channel_id(&self) -> ChannelId;
    pub fn session_id(&self) -> SessionId;
    pub fn security_context(&self) -> &SecurityContext;
    pub fn classification(&self) -> Classification;
    pub fn label(&self) -> Classification;
    pub fn see(&mut self, classification: &Classification);
    pub fn record_knowledge(&mut self, reference: KnowledgeRef);
    pub fn messages(&self) -> impl Iterator<Item = &Message>;
    pub fn stage_memory(&mut self, change: MemoryChange, origin: MemoryOrigin);
    pub fn staged_memory(&self) -> &[(MemoryChange, MemoryOrigin)];
    pub fn stage_note(&mut self, kind: NoteKind, change: NoteChange);
    pub fn staged_notes(&self, kind: NoteKind) -> &[NoteChange];
    pub fn set_plan(&mut self, plan: PlanState);
    pub fn plan(&self) -> Option<&PlanState>;
    pub fn take_plan(&mut self) -> Option<PlanState>;
    pub fn request_state(&mut self, state: ActorState);
    pub fn requested_state(&self) -> Option<ActorState>;
    pub async fn send_event(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()>;

    // mod.rs - for the actor
    pub(crate) fn tools(&self) -> &[Tool];
    pub(crate) fn event_context(&self) -> &EventContext;
    pub(crate) fn push_message(&mut self, message: Message);
    pub(crate) fn count_model_call(&mut self) -> bool;
    pub(crate) fn tool_calls(&mut self) -> ToolCalls<'_>;
    pub(crate) fn saw_private_knowledge(&self) -> bool;

    // ending.rs - both end the turn and give the borrow of the state back
    pub(crate) async fn commit(self, cancelled: bool) -> CommittedTurn;
    pub(crate) fn give_up(self, reason: GivenUpReason, names_only: Option<Classification>)
        -> CommittedTurn;

    // context.rs
    pub fn set_context(&mut self, parts: Vec<PromptPart>);
    pub(crate) fn full_context(&self) -> Result<(Vec<Message>, Vec<ToolDefinition>)>;
    pub(crate) fn place_turn_prompt(&mut self, role: MessageRole, text: String);

    // plan.rs
    pub(crate) fn plan_progress(&self) -> Option<PlanProgress>;
    pub(crate) fn in_plan_step(&self) -> bool;
    pub(crate) fn advance_plan(&mut self, next: usize) -> Option<ActorEvent>;
    pub(crate) fn plan_converged(&mut self) -> Vec<ActorEvent>;
}

/// What is still needed of a turn once it ended - taken over or given up.
pub struct CommittedTurn {
    pub final_messages: Vec<Message>,        // empty for a turn given up
    pub channel_state_changed: bool,
    pub answering: Answering,                // what its last events are sent with
    pub abandoned_calls: Vec<ToolCallState>, // whose approval requests are withdrawn
}
```

Gone from `Turn`: `new`, `next_turn`, `begin`, `current`/`current_mut` as a fallback to an empty
turn, `agent_channel_state_mut`, `set_processing_state`, `record_turn`, `keep_tools`, the field
`profile` (fixed in the turn's `ContextRef` when it begins), and the type `EndedTurn`. Whether one
type serves both endings, or giving up returns `(Answering, Vec<ToolCallState>)`, is settled in
step 4 by what reads better at the two call sites; no third type either way.

`OpenTask` and `RemoteState` (`caelumora-model/src/external_agent.rs`):

```rust
pub struct OpenTask {
    pub task: TaskId,
    pub since: DateTime<Utc>,
    pub state: OpenTaskState,
    pub unsaid: Option<String>,
    /// The envelope it answers, continued in the agent's name - as `TurnState::event_context`.
    #[serde(default, alias = "requested")]
    pub event_context: Option<EventContext>,
    /// Whose authority it acts with - as `TurnState::security_context`.
    #[serde(default)]
    pub security_context: Option<SecurityContext>,
}

impl RemoteState {
    pub fn is_empty(&self) -> bool;
    pub fn asking(&self) -> Option<&TaskId>;
    pub fn working(&self) -> impl Iterator<Item = &TaskId>;
    /// Records `task` as begun - or taken up again - in the contexts given.
    pub fn begin(&mut self, task: &TaskId, event_context: &EventContext,
        security_context: &SecurityContext);
    pub fn open(&mut self, task: &TaskId, state: OpenTaskState);
    pub fn close(&mut self, task: &TaskId);
    /// The contexts the open `task` is answered in, if they are kept.
    pub fn contexts_of(&self, task: &TaskId) -> Option<(EventContext, SecurityContext)>;
    pub fn unsaid_of(&self, task: &TaskId) -> Option<String>;
    pub fn set_unsaid(&mut self, task: &TaskId, unsaid: Option<String>);
}
```

`set_requested` and `requested_in` go. The classification a task began with is
[212](212-external-task-during-reclassification.md)'s to add, the `ContextRef`
[205](205-external-agent-message-context.md)'s.

Shared by both actors (`actor_impl/mod.rs`), beside `perceive` and `event_for`:

```rust
/// Takes in what the actor missed as what it hears, and stores the state.
async fn catch_up(state: &mut AgentChannelState, visible: Vec<Message>,
    takes: impl Fn(&Message) -> bool, processors: &[ComponentImpl<dyn PerceiveInProcessor>],
    security: &SecurityContext, runtime: &ActorRuntime) -> Result<()>;

/// What users are told of `error`: its message if it is one for them, else that the details
/// are in the server log.
fn failure_message(error: &anyhow::Error) -> String;
```

### Step 1 — `agent_turn/`, split by subject

`agent_turn.rs` becomes the module `agent_turn/` with `mod.rs`, `ending.rs`, `context.rs` and
`plan.rs`, each an `impl Turn` block with its helpers (`because`, `told_of_executed`; the plan's
two instructions) and its tests. Nothing is renamed, no signature changes; what a block needs of
another becomes `pub(super)`. The module comment is split along with it. AGENT-TURN.md names the
files.

### Step 2 — the turn's state and the state's rules, in the model

- `TurnState` and `TurnEntry` with their `impl` and their tests move to
  `caelumora-model/src/turn.rs`; the imports of the about ten files that name them follow.
- `Turn::record_turn` becomes `AgentChannelState::record_turn`, its warning with it.
- The mechanical half of `Turn::give_up_turn` - recording the entries, moving both heads back,
  `Transcript::give_up` - becomes `AgentChannelState::give_up_turn`. Abandoning the tool calls,
  the message told of what ran and its text stay in `Turn`.
- `Turn::next_turn` and `keep_tools` become `AgentChannelState::begin_turn`. This goes beyond the
  two methods first agreed for the small form: step 3 needs a turn to be begun on the state before
  there is a `Turn`, and the body is the state's alone. `Turn::next_turn` stays as a line that
  calls it until step 3.
- A test of each in the model: the chain of digests continued, both heads back where the turn
  began and the branch named, what a turn begun has seen. The tests in `agent_turn/` stay - they
  test through `Turn`.

No change of behaviour, none of what is stored.

### Step 3 — `Turn` only while a turn runs

- `Turn::of` replaces `Turn::new`: `None` between turns. `Turn` loses `profile`,
  `agent_channel_state_mut`, `set_processing_state`, `next_turn` and the test's `begin`; a helper
  for tests, `Turn::begun(agent, name, state)`, begins a turn with defaults and hands it out, and
  replaces the `Turn::new(…)` + `begin()` of about sixty tests one for one.
- The `AgentActor` works on its `AgentChannelState` where no turn is concerned:
  `handle_waiting`, `store_channel_state` and `settle` take the state; `request_response` begins
  the turn on the state (`begin_turn`) and then works with `Turn::of`. The run loop opens a `Turn`
  for the steps of a turn - `Planning`, `ExecutingTools`, `WaitingForApproval`, `Reflecting`,
  `Cancelling`, the check for a lowered channel - and for nothing else.
- `commit` and `give_up` take the turn by value: the state is free again to be stored.
- `prepare_resume` already makes sure that a state in a turn has its `TurnState`; a `Turn::of` that
  is `None` in a turn's step ends that step as a failed turn would, without a panic.
- What `current()` falls back to stays as a branch that cannot be reached: `unwrap` is not allowed,
  and `ContextResolver::of` reads the turn from the state, so `Turn` cannot hold it apart.

No change of behaviour. This is the step to review most closely: the order of going on, storing
and announcing in the run loop (issues 65, 75, 85, 152) must stay as it is.

### Step 4 — one `Answering`, and what both actors share

- `Answering` moves to `crate::actor` with `to`, `send` and `send_all`; the free `send` of
  `agent_turn` and `EndedTurn` go. `Turn::send_event` sends through the same function.
- `CommittedTurn` carries the `Answering` and the abandoned calls; giving up returns the same or
  the pair (see above).
- `catch_up` and `failure_message` in `actor_impl/mod.rs`, used by both actors.

No change of behaviour.

### Step 5 — `OpenTask` as the counterpart of `TurnState`

- `requested` is renamed `event_context` (read under its old name too), `security_context` is new;
  `RemoteState::begin` and `contexts_of` replace `set_requested` and `requested_in`.
- `resume`, `cancel` and what an open task says later (`speak`, `on_followed`) act in the task's
  contexts, as a resumed turn acts in its `TurnState`'s. A task stored without them acts as the
  agent itself, as today.
- **This changes behaviour:** after a restart, and between requests, the remote agent is called
  with the authority of the request that began the task - which decides, for one, whose secrets
  reach it - instead of the agent's own. A test in the security suite comes with it.
- CHANNELS.md and STORAGE.md say what an open task keeps.

### What to watch

- **Steps 1 to 4 change no behaviour** and nothing that is stored; step 5 changes both, and says so.
- **Few types:** new is none. `Answering` exists and moves; `EndedTurn` goes; `turn.rs` is a file,
  not a type.
- **Visibility:** the three new methods of `AgentChannelState` are `pub` in a crate the REST server
  and the TUI use too. That is the price of the rules being with the data; it is the same for
  `perceive` and `RemoteState` today.
- **Errors in a turn end the turn, not the agent** - also where the run loop finds no turn in a
  turn's step.
- **Not part of this issue:** an external task whose channel is lowered (212), the recorded
  context of an external agent's messages (205), work in the background for agents run here.

### What it costs

- Step 1: about 700 lines of code and 570 of tests move within the runtime; mechanical.
- Step 2: about 150 lines move to the model, three tests are new; imports in about ten files.
- Step 3: the run loop and five functions of `actor_impl/agent.rs` (about 150 lines touched), and
  about sixty test set-ups, mechanically. The largest step.
- Step 4: about 60 lines go, in both actors and `agent_turn`.
- Step 5: about 40 lines in the model and the external actor, one test in the security suite, two
  documents.

Each step is a commit of its own.

## Done when

- `Turn` cannot be built between turns, and nothing in it reads as empty for lack of one.
- Both actors hold their `AgentChannelState`, take in what they hear, catch up, publish and tell
  of a failure through the same functions.
- What keeps transcript and history consistent when a turn is begun, taken over or given up is a
  method of `AgentChannelState`; `TurnState` has a file of its own in the model.
- `agent_turn/` has a file per subject; no file of it holds more than one.
- An `OpenTask` keeps the contexts it acts in under the names a `TurnState` does, and the remote
  agent is called in them for as long as the task is open.
- AGENT-TURN.md, CHANNELS.md and STORAGE.md describe the cut.
- [206](206-one-agent-channel-state.md) is `done`: this is the refactoring it was kept open for.

## History

- **2026-10-03** — Issue created with the plan, after reading `Turn`, `AgentChannelState`,
  `TurnState`, `RemoteState` and `OpenTask` with the maintainer; all five steps decided. It is the
  refactoring agreed for the end of [206](206-one-agent-channel-state.md). Found on the way and
  given an issue of its own: [212](212-external-task-during-reclassification.md).
- **2026-10-03** — Step 1 done: `agent_turn.rs` is the module `agent_turn/` - `mod.rs` (the
  handle, what components work with, `next_turn` and `keep_tools` until step 2, the shared `send`),
  `ending.rs`, `context.rs`, `plan.rs` - each `impl Turn` block with its helpers and its tests;
  tests used by several files (`message`, `seen`) are in `mod.rs`. No signature changed. Deviating
  from the plan: `set_plan`, `plan` and `take_plan` are in `plan.rs` with the rest of the plan, not
  in `mod.rs`, so that the subject stays in one file.
- **2026-10-03** — Step 2 done: `TurnState` and `TurnEntry` are in `caelumora-model/src/turn.rs`;
  `AgentChannelState` has `begin_turn` (with what `keep_tools` did), `record_turn` and
  `give_up_turn`, each with a test in the model. `Turn::next_turn` is a line that calls
  `begin_turn` until step 3; `Turn::record_turn` and `keep_tools` are gone, and `give_up_turn`
  builds only what the agent is told before the state keeps the branch. Changed with it: the
  warning that a transcript changed during a turn no longer names the agent - the model does not
  know it; the agent's span, which every step of a turn runs in, does.
- **2026-10-03** — Step 3 done: `Turn::of` replaces `Turn::new` and is `None` between turns; the
  `profile` field, `next_turn`, `agent_channel_state_mut`, `set_processing_state`, `event_context`
  and the tests' `begin` are gone, `Turn::begun` begins one for a test. `commit_turn` and
  `give_up_turn` are `commit` and `give_up` and use the turn up. The `AgentActor` holds its state:
  waiting, perceiving, storing, settling, ending, failing and giving up a turn take the state, and
  the steps of a turn open it (`turn_under_way`); the check for a lowered channel and `turn_flow`
  read the `TurnState`. `logging::agent_span` takes the event context of a turn under way, if any.
  Changed with it: an error before a turn began - the actor has no channel, while it is torn
  down - is logged and no longer answered with a `TurnFailed` sent in empty contexts; a step of a
  turn that finds none fails as a turn's step, where it worked on an empty turn before. Tests that
  used a `Turn` without a turn under way (calculator, sleep, `create_plan`, the MCP integration)
  begin one now.
- **2026-10-03** — Step 4 done: `Answering` is in `crate::actor` (`to`, `send`, `send_all`) and
  publishes for both actors - the external agent's answers, the turn's events (`Turn::answering`,
  `Turn::send_event`) and what is sent once a turn ended; the free `send` of `agent_turn` and
  `EndedTurn` are gone. `CommittedTurn` (now `pub(crate)`) carries the `Answering` and the
  abandoned calls; `Turn::give_up` returns the pair, as a tuple. `withdraw_approval_requests` is a
  free function of the two. `catch_up` and `failure_message` are in `actor_impl` and used by both
  actors; the `AgentActor` takes the request envelope to begin a turn (`Answering::to`), as the
  external one does. Changed with it: a failed send names the actor in the same words for both
  ("actor … failed to send an event").

