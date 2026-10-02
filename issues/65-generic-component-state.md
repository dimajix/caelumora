# 65 — Component state is hard-wired into the agent state and the turn

**Status:** open
**Area:** Context & knowledge

## The problem

Notes ([17](17-information-sharing-within-session.md)) and private notes
([51](51-agent-private-notes.md)) are features of components (`notes`, `private_notes`), but their
state is built into the core types: `AgentChannelState` has the fields `notes`, `turn_notes`,
`private_notes` and `turn_private_notes`; `TurnContext` has `note`, `revoke_note`, `private_note`,
`revoke_private_note`, `notes_digest`, ...; `record_turn` knows that exactly these two chains are to
be taken over at the end of a turn; `ChannelLoop::published_notes`, `SessionReader::private_notes`
and the REST snapshot know them by name. Every further kind of state a component needs - a
scratchpad, a task list, a document, counters, a component's own memory - would add fields and
methods to the data model and the turn again.

## Why it matters

- Components are the extension point of the system; state that only the core can hold means a new
  capability needs changes to the model, the turn, the stores and the REST layer - not just a
  component.
- The core types grow with every feature and know about all of them.
- Branching ([08](08-retry-and-edit-past-answers.md)) has to treat every kind of state consistently:
  turning the chains into trees, resetting on a branch switch, recording provenance (principles 1,
  5, 6 in [19](19-memory-tiers.md)). Done per hard-wired field, every new kind is another place
  branching has to be taught - and forgotten.

## Considerations

- **A generic store of component state** in `AgentChannelState` (and maybe `AgentState` for state
  across channels, [51](51-agent-private-notes.md)), keyed by component id (or a kind/key the
  component declares), holding serialised values the core does not interpret.
- **Chains as the generic shape.** Notes are a `Chain` of entries with digests
  ([48](48-shared-digest-chain.md)); history and context are too. If component state is kept as
  chains of changes (events) rather than as overwritten values, branching can treat all of it alike:
  fork at a message, switch the head, take a turn's changes over at its end. A component would
  supply how to fold its entries into its current state.
- **The turn stays generic.** `TurnContext` offers access to "the state of component X" - read the
  current state, stage changes for this turn - without knowing what the state is. `record_turn`
  commits all staged changes of all components at once, anchored at the turn's last message, as it
  does today for the two note chains.
- **Reading across channels and actors** (`published_notes`, `private_notes` of other channels) must
  become generic as well - with the confidentiality rules ([61](61-reasoning-leaves-the-agent.md))
  applied by the core, not re-implemented per component.
- **Events and REST:** `NotesChanged` and the notes in the snapshot are specific too; a generic
  "state of component X changed" event, with the component deciding what users see, or a component
  facet that renders its state for users.
- **Typed access for components** over an untyped store (serde values behind a typed handle), so
  that components do not handle raw JSON.
- Provenance ([19](19-memory-tiers.md), principle 5) and digests of the state must stay recordable
  in `KnowledgeRef` without the core knowing the kinds.

## Done when

- Components can keep state in the agent state without the data model, `TurnContext` or the stores
  knowing its kind.
- Notes and private notes use that mechanism, and branching can handle all component state the same
  way.

## History

- **2026-09-27** — Issue created at the maintainer's request: the agents' notes should be stored
  generically, in the state and in the `TurnContext`; further kinds of state will be defined
  implicitly by components, and neither the core classes nor the data model should know them. The
  mechanism must support branching conversations later.
- **2026-09-28** — The agent memory of [62](62-json-store-concurrent-writes.md) (step 3) sets the
  pattern notes are to follow here: `TurnContext` offers generic access to a state (a handle, staged
  changes, commit when the turn completes) and knows nothing of what the state is; the logic of a
  kind of state lives in a wrapper (`AgentMemory`) used by the component and by anyone else, not in
  `TurnContext`. Today notes still have `note`, `revoke_note`, `private_note`, `revoke_private_note`
  and their digests in `TurnContext` and are taken over in `record_turn` by name.
- **2026-09-28** — Asked by the maintainer whether `AgentChannelState` should follow the pattern of
  `AgentState` ([62](62-json-store-concurrent-writes.md), step 3). Decided: not as a whole, but
  split.
  - The lost-update argument does not apply: `AgentChannelState` has exactly one writer, the actor
    of that agent and channel (true since sessions start only once,
    [62](62-json-store-concurrent-writes.md) step 1). Handle, snapshot and deltas were needed for
    `AgentState` because several actors change it.
  - **The actor's working state stays as it is:** the turn's messages, pending tool calls, plan,
    requested state, scratch - mutable state of one actor during one step, where snapshots and
    deltas would cost without gain.
  - **Component state in it follows the pattern:** notes, private notes and later kinds get the same
    shape as agent state - generic access in `TurnContext`, staged changes, taken over when the turn
    completes, the logic in a wrapper - stored per agent and channel instead of per agent. That is
    this issue.
  - A handle for `AgentChannelState` would let other channels read another's notes from memory
    instead of the `StateStore` (`published_notes`, `private_notes`); convenient, optional.
- **2026-09-28** — First intermediate step decided with the maintainer: notes are treated like agent
  memory.
  1. `caelumora-runtime/src/notes.rs` is split: it is mostly the `SessionReader` (reading across
     channels - notes, private notes, and histories), which becomes `session_reader.rs`; the notes
     logic becomes a wrapper `AgentNotes` in `agent_notes.rs`, like `AgentMemory`, with what
     `TurnContext` had for notes (`note`, `revoke_note`, `private_note`, ...).
  2. The `SessionReader` leaves `TurnContext`: it is given to the components that need it (`notes`,
     `private_notes`, `channel_history`) when they are built, through `ComponentContext` (`None` for
     session-level components such as commands).
  3. Generic, like `AgentState` but without a handle (one writer): the model has
     `AgentChannelChange` and `AgentChannelState::apply`; a turn stages changes in one list instead
     of `turn_notes` / `turn_private_notes`, and `record_turn` applies them without knowing what
     they are.
  4. `turn_scratch` is removed: it was never used.
  5. `ActorEvent::NotesChanged` is replaced by two generic events without content,
     `GlobalEvent::AgentStateChanged(AgentId)` - published with the agent's attributes by the
     `AgentStateHandle` after storing - and `GlobalEvent::AgentChannelStateChanged(SessionId,
     ChannelId, AgentId)` - published with the channel's attributes by the agent after storing its
     channel state, which now happens before `FullResponseGenerated`. Whoever may see more fetches
     it (the web UI reloads the channel's notes). The state is private in part (memory, private
     notes), so the events say only that it changed; which kind changed is for later. Storing before
     announcing the end of a turn also closes a gap for other agents, which read notes from the
     store when their turn begins.
- **2026-09-28** — First intermediate step implemented:
  - `session_reader.rs` (was `notes.rs`) with the `SessionReader`, plus
    `session_reader::detached()`, a reader that reads nothing; `agent_notes.rs` with the wrapper
    `AgentNotes` (`NoteKind::Public` / `Private`).
  - The reader is a required part of `ComponentContext` (the maintainer's request: not optional);
    the agent gives its components a `ChannelSessionReader`, the session's commands get
    `detached()`. The `notes`, `private_notes` and `channel_history` components hold it; their "no
    other channels can be read here" branches are gone. Giving components the `ActorRuntime`
    instead, as the maintainer proposed, waits for [69](69-scoped-actor-runtime.md): as it is, it
    would let them read around the confidentiality rules.
  - `AgentChannelChange` / `AgentChannelState::apply`; `TurnContext` has `agent_channel_state()`,
    `stage_channel_change` and `staged_channel_changes` instead of the note methods, `record_turn`
    applies the staged changes without knowing them. `turn_notes`, `turn_private_notes` and
    `turn_scratch` are gone.
  - `ActorEvent::NotesChanged` is replaced by `GlobalEvent::AgentStateChanged` (announced by the
    `AgentStateHandle` after storing) and `GlobalEvent::AgentChannelStateChanged` (announced by the
    agent in `reflect` after storing its channel state, which now happens before
    `FullResponseGenerated`). The web UI's notes panel listens to the global stream (handed on by
    the session list through `globalEvents.ts`) and fetches the notes from the new `GET
    …/channels/{cid}/notes`.
- **2026-09-28** — Verified in the browser: an agent's public note appears in the notes panel
  without reloading the page - the web UI fetched the notes on `AgentChannelStateChanged`; other
  channels' panels stay empty, as notes are per channel.
- **2026-09-28** — Components now take the `SessionReader` from the actor's runtime
  ([69](69-scoped-actor-runtime.md)): `ComponentContext` carries `Option<ActorRuntime>`,
  `ComponentContext::session_reader()` builds the reader from it.
- **2026-09-30** — First step, from [92](92-ledger-for-notes-and-memory.md): notes and memory are
  one type, `Ledger<K>`, instantiated per kind. The direction for the rest of this issue is that the
  core does not know the kind - not another case in `AgentChannelChange` for the next component's
  state.
- **2026-10-01** — From [156](156-notes-and-memory-staged-scans.md): `AgentChannelChange` keeps its
  two variants with the same payload (`Note`, `PrivateNote`), and `NoteKind::change` and
  `NoteKind::of` in `agent_notes.rs` map between them. Folding them into `AgentChannelChange::Note {
  kind, change }` was deliberately left to this issue rather than done in passing.
