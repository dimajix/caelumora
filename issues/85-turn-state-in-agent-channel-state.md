# 85 — The turn's working state is eleven loose fields of `AgentChannelState`

**Status:** open
**Area:** Code structure

## The problem

Of the 19 fields of `AgentChannelState` (`caelumora-model/src/agent.rs:175-247`),
eleven are the working state of one turn: `turn_context`, `turn_messages`,
`turn_knowledge`, `turn_publication`, `turn_classification`,
`turn_agent_changes`, `turn_channel_changes`, `turn_security_context`,
`turn_event_context`, `tool_calls`, `plan`, `requested_state`. Three
overlapping lists reset them by hand: `next_turn` (`agent_turn.rs:152-158`),
`abandon_turn` (`124-129`), `clear_work` (`133-138`). "No turn is running" is
a convention over eleven fields, not a fact the type states. The document
also holds a synchronisation primitive:
`processing_state: Arc<ProcessingStateHolder>` with its own serde module
(`actor.rs:439-465`), which the actor clones (`agent.rs:116`).

## Why it matters

- Whether a turn was under way - and should be resumed after a crash
  ([75](75-agent-resumes-half-turn.md)) - is a fact about eleven fields, not
  one.
- Issue [08](08-retry-and-edit-past-answers.md) discards the working state
  on a branch switch; issue [65](65-generic-component-state.md) keeps the
  working state mutable and stages the component state - both want the line
  between the two drawn in the type.
- Every new turn field is a fourth place to forget in a reset list.

## Considerations

- `pub turn: Option<TurnState>` with a sub-struct holding the eleven
  fields. `next_turn` = `Some(TurnState::new(event_context, security_context,
  classification, publication))`; `commit_turn`/`abandon_turn` = `None`
  plus what they take over. A stored `Some` is a turn to resume after a
  crash; a deliberate stop abandons the turn before storing, so it leaves
  `None`.
- `processing_state` becomes a plain `ActorState` in the document; the
  actor owns its `ProcessingStateHolder` and writes the value in when it
  stores. `processing_state_serde` and `default_processing_state` go.
- `Turn` keeps its shape; its methods reach `self.state.turn` instead of
  eleven fields (~60 accesses, all in `agent_turn.rs`). `channel_id()`/
  `session_id()` come from the classification as today.
- Alternative considered: keep the fields, merge the three reset lists into
  one. Cheaper, but leaves "no turn" a convention. Not recommended.

## Done when

- `AgentChannelState` has one field for the turn's working state, and it is
  `None` between turns.
- No `Arc` or atomic is serialised.
- Loading a state with `turn: None` yields an idle actor; with `turn: Some`
  the actor resumes as 75 specifies.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.1.3 and 3.1.6). Asked for explicitly
  by the maintainer.
