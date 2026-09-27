# 65 — Component state is hard-wired into the agent state and the turn

**Status:** open
**Area:** Context & knowledge

## The problem

Notes ([17](17-information-sharing-within-session.md)) and private notes
([51](51-agent-private-notes.md)) are features of components (`notes`,
`private_notes`), but their state is built into the core types:
`AgentChannelState` has the fields `notes`, `turn_notes`, `private_notes`
and `turn_private_notes`; `TurnContext` has `note`, `revoke_note`,
`private_note`, `revoke_private_note`, `notes_digest`, ...; `record_turn`
knows that exactly these two chains are to be taken over at the end of a
turn; `ChannelLoop::published_notes`, `SessionReader::private_notes` and
the REST snapshot know them by name. Every further kind of state a
component needs - a scratchpad, a task list, a document, counters, a
component's own memory - would add fields and methods to the data model and
the turn again.

## Why it matters

- Components are the extension point of the system; state that only the
  core can hold means a new capability needs changes to the model, the
  turn, the stores and the REST layer - not just a component.
- The core types grow with every feature and know about all of them.
- Branching ([08](08-retry-and-edit-past-answers.md)) has to treat every
  kind of state consistently: turning the chains into trees, resetting on a
  branch switch, recording provenance (principles 1, 5, 6 in
  [19](19-memory-tiers.md)). Done per hard-wired field, every new kind is
  another place branching has to be taught - and forgotten.

## Considerations

- **A generic store of component state** in `AgentChannelState` (and maybe
  `AgentState` for state across channels, [51](51-agent-private-notes.md)),
  keyed by component id (or a kind/key the component declares), holding
  serialised values the core does not interpret.
- **Chains as the generic shape.** Notes are a `Chain` of entries with
  digests ([48](48-shared-digest-chain.md)); history and context are too.
  If component state is kept as chains of changes (events) rather than
  as overwritten values, branching can treat all of it alike: fork at a
  message, switch the head, take a turn's changes over at its end. A
  component would supply how to fold its entries into its current state.
- **The turn stays generic.** `TurnContext` offers access to "the state of
  component X" - read the current state, stage changes for this turn -
  without knowing what the state is. `record_turn` commits all staged
  changes of all components at once, anchored at the turn's last message,
  as it does today for the two note chains.
- **Reading across channels and actors** (`published_notes`,
  `private_notes` of other channels) must become generic as well - with the
  confidentiality rules ([61](61-reasoning-leaves-the-agent.md)) applied by
  the core, not re-implemented per component.
- **Events and REST:** `NotesChanged` and the notes in the snapshot are
  specific too; a generic "state of component X changed" event, with the
  component deciding what users see, or a component facet that renders its
  state for users.
- **Typed access for components** over an untyped store (serde values
  behind a typed handle), so that components do not handle raw JSON.
- Provenance ([19](19-memory-tiers.md), principle 5) and digests of the
  state must stay recordable in `KnowledgeRef` without the core knowing the
  kinds.

## Done when

- Components can keep state in the agent state without the data model,
  `TurnContext` or the stores knowing its kind.
- Notes and private notes use that mechanism, and branching can handle all
  component state the same way.

## History

- **2026-09-27** — Issue created at the maintainer's request: the agents'
  notes should be stored generically, in the state and in the
  `TurnContext`; further kinds of state will be defined implicitly by
  components, and neither the core classes nor the data model should know
  them. The mechanism must support branching conversations later.
