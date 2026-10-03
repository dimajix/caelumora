# 206 — Agents run here and external agents keep their channel state in two types

**Status:** in progress
**Area:** Code structure

## The problem

An agent's state in a channel is one of two types, stored and read apart:

| | `AgentChannelState` (agent run here) | `ExternalAgentChannelState` (issue 179) |
|---|---|---|
| shared | `history: MessageHistory`, `transcript: Transcript` | `history: MessageHistory`, `transcript: Transcript` |
| own | `processing_state`, `notes`, `private_notes`, `prompts`, `tool_sets`, `turn` | `offered_up_to`, `conversation`, `open_tasks` |

The parts they share are the same types with the same meaning - what the agent heard and said, and
what it was given - but everything built on them exists twice or for one side only:

- the `StateStore` has `get_/put_agent_channel_state` and `get_/put_external_agent_channel_state`,
  each with a change type of its own (`AgentChannelStateChange`, `ExternalAgentChannelStateChange`,
  issue 193);
- the views read them through two `AgentMember` implementations and two `AgentContextView`s
  (`InternalContext`, `ExternalContext`), of which the external one answers most questions with
  nothing;
- `ContextResolver::of` and `TranscriptView::of` take an `AgentChannelState`, so a context
  recorded for an external agent ([205](205-external-agent-message-context.md)) cannot be resolved
  without changing them;
- catching up on what was missed (`catch_up`, issue 190) is written once per actor kind.

## Why it matters

- Every capability built on the history or the transcript - showing contexts (205), given-up
  turns, branches (issue 08), notes an external agent might one day keep - has to be built twice
  or is missing for external agents.
- The difference between the two kinds is how an agent produces its answers, not what it keeps of
  the channel; two state types make it look like the latter.

## Considerations

- **One state with a part per kind.** `AgentChannelState` keeps what every agent keeps - history,
  transcript, perhaps notes, prompts and tool sets (empty for an external agent) - and an enum or
  an optional part for what only one kind has: the turn of an agent run here, the conversation,
  open tasks and `offered_up_to` of an external one. One pair of store methods, one change type,
  one context view.
- **A shared core, two states.** A type for the shared part that both states contain, and
  resolvers, views and store helpers written against it. Less of a change, but the store and the
  change types stay doubled.
- **Stepwise.** First make what reads the shared parts (resolver, transcript view, context view)
  take them instead of a whole `AgentChannelState` - which 205 needs anyway - then merge the
  states.

Stored states of external agents may be deleted rather than migrated, as decided for
[201](201-component-nodes-wrapper.md), unless decided otherwise here.

## Plan

Worked out on 2026-10-03, from the code as it stands. Three questions in it are open (*Open
questions*); the plan is written for the recommended answers and says where another answer changes
it.

### What is affected

**`caelumora-model`**

- `agent.rs`: `AgentChannelState`, `AgentChannelStateChange` and its `Journaled` implementation
  gain the part of an external agent.
- `external_agent.rs`: `ExternalAgentChannelState`, `ExternalAgentChannelStateChange` and their
  `Journaled` implementation go; what stays is what only an external agent keeps
  (`ConversationId`, `TaskId`, `OpenTask`, and the part that holds them).
- `store.rs`: `get_/put_external_agent_channel_state` go.
- `store_impl/json.rs`, `store_impl/memory.rs`: the second directory
  (`runtime/external_agent_channel_state`), the second map, their path helpers and the second
  delete in `delete_channel_state` go. `store_impl/suite.rs` loses the half that tests the second
  pair, and "the two states of one agent are kept apart".
- `context.rs`: nothing. `ContextResolver::of` and `TranscriptView::of` take an
  `AgentChannelState`, which is then the state of every agent - the change
  [205](205-external-agent-message-context.md) planned for them is not needed.

**`caelumora-runtime`**

- `runtime.rs`: `ActorRuntime::load_/store_external_channel_state` go; both actors use
  `load_/store_channel_state`.
- `actor_impl/external_agent.rs`: the actor holds a `Mutex<AgentChannelState>`. 22 mentions of
  the type; its helpers (`set_open`, `unsaid_of`, `requested_in`, `set_requested`, `set_unsaid`,
  `close`, `unoffered`) read the external part instead of the whole state.
- `actor_impl/agent.rs`, `agent_turn.rs`: unchanged, unless taking in what others said is shared
  (question 2).
- `view/context.rs`: `InternalContext` and `ExternalContext` become one. `AgentContextView` then
  has one implementation, and is a struct rather than a trait.
- `view/member.rs`: `InternalAgentMember` and `ExternalAgentMember` become one; the `AgentMember`
  trait, the `Personal` pair and the `match` on `PersonalityKind` go. The personality is read for
  the introduction only - the views no longer decide an agent's kind, which removes the second
  of the two places of issue [189](189-agent-kind-decided-twice.md) altogether.
- `view/channel.rs`, `caelumora-rest-server`: only the return type of `context()`, if the trait
  becomes a struct. The REST API and the web UI do not change.
- Tests: `view/mod.rs` (`the_context_of_an_external_agent_...`, which stores a "stray"
  `AgentChannelState` to show the two are kept apart - that case no longer exists), the tests of
  `external_agent.rs` and of the external agent in `channel.rs`.

**Docs**: STORAGE.md (*The state store*, *What a state tells a store*, *One writer per state*),
CHANNELS.md (external agents), AGENT-TURN.md (*Asking what an agent saw*).

### The one state

Both kinds keep the same things of the channel, and each has one part the other has not: an agent
run here its turn under way, an external agent what ties its next exchange to the last. The turn
is an optional part of the state already; the external part is added beside it, the same way.

```rust
pub struct AgentChannelState {
    pub processing_state: ActorState,
    pub history: MessageHistory,
    pub transcript: Transcript,
    pub notes: NoteChain,
    pub private_notes: NoteChain,
    pub prompts: Kept<String>,
    pub tool_sets: Kept<Vec<ToolDefinition>>,
    /// The turn under way of an agent run here, `None` between turns.
    pub turn: Option<TurnState>,
    /// What an external agent keeps of the remote side, `None` for an agent run here.
    pub remote: Option<RemoteState>,
}

/// In `external_agent.rs`: what was `ExternalAgentChannelState` without history and transcript.
pub struct RemoteState {
    pub offered_up_to: Option<MessageId>,
    pub conversation: Option<ConversationId>,
    pub open_tasks: Vec<OpenTask>,
}
```

- **One change type.** `AgentChannelStateChange` gains one variant, `Remote(Option<RemoteState>)`,
  recorded whole like `Turn`: three small values, bounded by the open tasks. The three variants of
  today (`OfferedUpTo`, `Conversation`, `OpenTasks`) become one.
- **The helpers on tasks become methods of `RemoteState`** (`open`, `close`, `unsaid_of`,
  `requested_in`, ...): six free functions in the actor that each search `open_tasks` move to the
  type they are about.
- **What an external agent leaves empty** - `notes`, `private_notes`, `prompts`, `tool_sets`,
  `turn` - needs no code: the one view answers "no notes", "no given-up turns", "no pending
  approvals" from the empty parts, where `ExternalAgentMember` and `ExternalContext` spell each
  out today.
- **The store does not learn the kind.** A state that was never stored is the default for both
  kinds; `remote` is set by the `ExternalAgentActor` when it first has something to keep.
- **`processing_state` stays `Idle` for an external agent.** A deliberate difference: an agent run
  here is resumed in the middle of a turn from it; an external agent is resumed from its open
  tasks, and where its actor stands (`Planning` while the remote agent works) lives only as long
  as the exchange does.

### What to watch

- **Stored states of external agents** lie under `runtime/external_agent_channel_state` and are
  not read any more. Deleting them (as allowed above) loses the remote conversation and the open
  tasks: the remote agent begins a new conversation, and is given the whole visible history again
  with the next request, since `catch_up` (issue 190) refills the history and nothing is marked
  as offered. Tasks left working on the remote side are neither followed nor cancelled.
- **The journal** (issue 193) gains one record per put for the new part, also for agents run here
  (`"remote": null`) - as `Turn` and `ProcessingState` are written with every put today. Whether
  the small parts should only be written when they changed is a question of its own, not of this
  issue.
- **Serde**: `remote` is `#[serde(default)]`, so stored states of agents run here still load.
- **An agent whose personality changes kind** kept two states before, one per kind; now it keeps
  one, and an `AgentActor` would find a transcript that holds only what the remote agent was
  given. Not handled today either (the other state simply started empty); it is named in
  CHANNELS.md rather than solved here.
- **`AgentChannelStateChanged`** is announced by the `AgentActor` only. With one state and one
  view, an external agent's context and history become things a client shows (205,
  [207](207-agent-history-in-ui.md)) and would reload on that event; announcing it from the
  `ExternalAgentActor` belongs to those issues, and is one call once the storing is shared.
- **Security**: who may read the state does not change (`ReadAgentContext` on the agent). No new
  action; the security suite needs no new case, only the existing ones for an external agent's
  context to keep passing.
- **The differences between the two actors that have no reason** show once the state is one:
  - Storing: `AgentActor::store_channel_state` logs a failure in one place; the
    `ExternalAgentActor` has five call sites, each with its own `warn!`. One method per actor.
  - Perceive processors that fail: an agent run here logs it and keeps the messages out of its
    transcript for good; an external agent fails the answer and offers them again next time.
    To be aligned with question 2, in one direction or the other.

### Open questions

1. **A part beside the turn, or an enum of kinds?** The plan adds `remote: Option<RemoteState>`
   beside `turn: Option<TurnState>`. The alternative is one field
   `kind: Internal { turn } | External { remote }`, which makes "a turn and a remote state at
   once" unrepresentable. It costs more than it gives: the store would have to know the kind to
   build the default, every reader of `state.turn` (72 mentions in `agent_turn.rs` alone) would
   match first, and notes, prompts and tool sets would stay outside the enum although only one
   kind fills them. **Recommended: the part beside the turn.**

2. **Is what others said taken in the same way by both kinds?** Today it is not:
   - an agent run here puts all it hears into its history, runs its perceive processors at once
     and puts what passes into its transcript (`AgentActor::perceive`);
   - an external agent puts all it hears into its history and nothing else. Only when it is asked,
     it takes what came after `offered_up_to`, filters it (`worth_giving`), runs the processors
     and puts what was *sent* into its transcript (`exchange`, `unoffered`).

   Made the same, an external agent would filter and run its processors when it hears, too. Its
   transcript is then what the remote agent *is* given, as for an agent run here, and a mark in
   it - the digest up to which it was sent, replacing `offered_up_to` - says how far the remote
   agent has it. `perceive` and `catch_up` are then written once for both actors, `unoffered` goes,
   and the context of a message (205) is exactly the entries between two marks. It is a change of
   behaviour: processors see the messages as they arrive, not batched per call, and the
   transcript holds, for a while, what has not left the system yet - so who reads it must be told
   by the mark what has. **Recommended: yes, as a step of its own (step 4), after the merge.**
   Without it, `catch_up` stays written twice, and "Done when" is met only for the context and
   given-up turns.

3. **Before or after 205?** 205 planned to make `ContextResolver::of` and `TranscriptView::of`
   take the parts they read, as a step towards this issue. With the states merged first, that step
   is not needed: the external agent records a `ContextRef`, and the one view resolves it.
   **Recommended: steps 1 to 3 of this issue first, then 205, then step 4** - or step 4 before
   205, if question 2 is answered with yes, since 205's reference is simplest against the mark.

### Steps

Each step compiles, passes the tests and can be reviewed on its own. Steps 1 and 2 restructure
without changing behaviour; step 4 changes behaviour.

1. **The external part becomes a type of its own.** `RemoteState` is cut out of
   `ExternalAgentChannelState` (which keeps `history`, `transcript`, `remote: RemoteState`), the
   three change variants become one, and the six task helpers of the actor become its methods.
   Nothing moves between stores yet. Small; `external_agent.rs` in both crates only.
2. **One state.** `AgentChannelState` gains `remote`; `ExternalAgentChannelState`, its change
   type, the second pair of `StateStore` methods, the second directory and map, and
   `ActorRuntime::load_/store_external_channel_state` go. The `ExternalAgentActor` holds an
   `AgentChannelState`. The store suite and the JSON store's tests are adjusted. Medium: mostly
   deletions, and mechanical changes in the actor.
3. **One view.** `InternalContext` and `ExternalContext` become one `AgentContext`,
   `InternalAgentMember` and `ExternalAgentMember` one; the `AgentMember` trait and the `match` on
   `PersonalityKind` in `view/member.rs` go, and `AgentContextView` becomes a struct if nothing
   else implements it by then. The view test with the stray state is rewritten to what still
   holds: an external agent's context is what the remote agent was given. Small.
4. **One way of taking in what others said** (if question 2 is answered with yes). A method of the
   state takes what was offered and what passed the processors - history, transcript, and what a
   turn under way has seen; one function in the runtime runs the processors before it, used by
   both actors' `catch_up` and by what they hear. `offered_up_to` becomes the mark in the
   transcript, `unoffered` goes, and the two actors fail the same way when a processor fails.
   Medium; touches `agent_turn.rs` (`extend_history`, `extend_transcript`), both actors and the
   external agent's tests in `channel.rs`.
5. **Docs and records.** STORAGE.md, CHANNELS.md, AGENT-TURN.md; this issue and INDEX.md; a note
   in 205 that its resolver step is not needed any more, and in 189 that the views no longer read
   the kind.

### What it costs

- Steps 1 to 3 remove more than they add: roughly 250 lines go (`ExternalAgentChannelState` with
  its change type and `Journaled`, the store methods in the trait and two implementations, two
  runtime methods, one context view, one member view, the trait) against roughly 60 new ones
  (`RemoteState` and its methods, the change variant). About 15 files, 5 of them docs.
- Step 4 is the only part with a risk: it changes when processors run and what the transcript of
  an external agent means, and needs new tests for the mark (nothing sent twice after a failed
  send, nothing skipped after a restart between hearing and sending).
- Stored states of external agents are lost (see *What to watch*).
- No change to the REST API, the web UI or the seed and configuration formats.
- What is not gained: the two actors stay two. Their run loops differ for a reason - one runs
  turns, the other follows remote tasks - and merging them is not part of this issue.

## Done when

- Agents run here and external agents keep their channel state in one type, stored through one
  pair of `StateStore` methods with one change type.
- What reads history and transcript - context, given-up turns, catching up - is written once.
- STORAGE.md and CHANNELS.md describe the one state.

## History

- **2026-10-03** — Issue created as the target named in
  [205](205-external-agent-message-context.md): reuse what exists for the context of an external
  agent's message, and perhaps merge the two states.
- **2026-10-03** — Plan worked out (*Plan*): one `AgentChannelState` with the external agent's part
  (`RemoteState`) beside the turn, in five steps, of which the first three only restructure. Open:
  the shape of the kind-specific part, whether both kinds take in what others said the same way,
  and the order with 205.
- **2026-10-03** — Decided, as recommended in the plan's *Open questions*:
  1. The external agent's part is a field `remote: Option<RemoteState>` beside `turn`, not an enum
     of kinds.
  2. Both kinds take in what others said the same way: an external agent filters and runs its
     perceive processors when it hears, and a mark in its transcript replaces `offered_up_to`
     (step 4).
  3. Order: steps 1 to 3, then step 4, then [205](205-external-agent-message-context.md), whose
     reference is recorded against the mark.
- **2026-10-03** — Step 1 done: `RemoteState` (`offered_up_to`, `conversation`, `open_tasks`) is a
  part of `ExternalAgentChannelState` of its own, recorded whole as one change (`Remote`) instead of
  three, and the task helpers of the `ExternalAgentActor` are its methods (`asking`, `working`,
  `open`, `close`, `unsaid_of`, `set_unsaid`, `requested_in`, `set_requested`). No change of
  behaviour; stored states of external agents from before cannot be read and are to be deleted.
- **2026-10-03** — Step 2 done: `AgentChannelState` holds an external agent's part
  (`remote: Option<RemoteState>`, read through `remote()` and changed through `remote_mut()`) and is
  the state of both kinds. `ExternalAgentChannelState` with its change type, the second pair of
  `StateStore` methods with the JSON store's directory `runtime/external_agent_channel_state` and
  the memory store's map, and `ActorRuntime::load_/store_external_channel_state` are gone. The views
  still read the state per kind; that is step 3. Stored states of external agents under the old
  directory are no longer read and are to be deleted.
- **2026-10-03** — Decided after trying both: `remote` is a plain `RemoteState`, not an `Option`
  (stored only when not empty, `RemoteState::is_empty`), and is read and changed directly, as
  `notes` or `prompts`; `remote()` and `remote_mut()` are gone. Its `None` meant nothing an empty
  part does not, and the accessors only made up for that. The turn stays an `Option`: its `None`
  means "between turns", and `Turn` encapsulates it.
- **2026-10-03** — To be looked at in a refactoring at the end of this issue: the counterpart of
  `TurnState` for an external agent is the `OpenTask`, not `RemoteState`. Unlike a turn, it does not
  keep in whose name it was begun - `resume` and `speak` act in the agent's own `SecurityContext`,
  not in the chain of the request that began the task, though its `EventContext` is kept
  (`requested`, issue 45) - nor the channel's classification when it began. Its `ContextRef` is
  [205](205-external-agent-message-context.md)'s.
- **2026-10-03** — Step 3 done: `AgentContextView` is one struct instead of a trait with
  `InternalContext` and `ExternalContext`, and `AgentMember` one struct instead of a trait with a
  member per kind; `MemberState::agent` no longer reads the personality and cannot fail. The
  personality is read for the introduction alone, so the views no longer decide an agent's kind
  (issue [189](189-agent-kind-decided-twice.md)). Changed with it: an agent whose personality is
  gone keeps its state in the views - its notes, its context - where it had none, since which kind
  of state it kept was unknown.
- **2026-10-03** — Step 4 done: both kinds take in what they hear the same way - in
  `AgentChannelState::perceive` (history, transcript, what a turn under way has seen) after
  `actor_impl::perceive` ran the perceive processors - when they hear it and when they catch up;
  `Turn::extend_history` and `extend_transcript` are gone, and the `AgentActor` needs no `Turn` to
  catch up. An external agent's transcript is now what passed its processors when it heard it, and
  `RemoteState::sent_up_to`, a digest of its transcript, replaced `offered_up_to`: a call gives the
  remote agent what the transcript gained since (`Transcript::messages_after`); `unoffered` is gone.
  Changed with it: an external agent's processors see messages as they arrive, not batched per call,
  and a failing stage no longer fails its answer but is logged and drops what it was given, as for
  an agent run here. Stored states of external agents from before resend their whole transcript
  once; they are to be deleted.
- **2026-10-03** — Step 5 done: STORAGE.md, CHANNELS.md and AGENT-TURN.md describe the one state,
  the one view and the one way of taking in what is heard; no document or code names the removed
  types any more. All points of *Done when* are met. Left open on purpose, as agreed: a refactoring
  at the end of this issue, beginning with the `OpenTask` as the external agent's counterpart of
  `TurnState` (see the entry above on it). Hence `in progress`, not `done`.
