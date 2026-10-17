# 85 — The turn's working state is eleven loose fields of `AgentChannelState`

**Status:** done
**Area:** Code structure
**Epic:** [74](74-refactoring-epic.md)
**Summary:** The working state of a turn is eleven loose fields of `AgentChannelState`, reset by three overlapping lists.

## The problem

Of the 19 fields of `AgentChannelState` (`caelumora-model/src/agent.rs:175-247`), eleven are the
working state of one turn: `turn_context`, `turn_messages`, `turn_knowledge`, `turn_publication`,
`turn_classification`, `turn_agent_changes`, `turn_channel_changes`, `turn_security_context`,
`turn_event_context`, `tool_calls`, `plan`, `requested_state`. Three overlapping lists reset them by
hand: `next_turn` (`agent_turn.rs:152-158`), `abandon_turn` (`124-129`), `clear_work` (`133-138`).
"No turn is running" is a convention over eleven fields, not a fact the type states. The document
also holds a synchronisation primitive: `processing_state: Arc<ProcessingStateHolder>` with its own
serde module (`actor.rs:439-465`), which the actor clones (`agent.rs:116`).

## Why it matters

- Whether a turn was under way - and should be resumed after a crash
  ([75](75-agent-resumes-half-turn.md)) - is a fact about eleven fields, not one.
- Issue [08](08-retry-and-edit-past-answers.md) discards the working state on a branch switch; issue
  [65](65-generic-component-state.md) keeps the working state mutable and stages the component
  state - both want the line between the two drawn in the type.
- Every new turn field is a fourth place to forget in a reset list.

## Considerations

- `pub turn: Option<TurnState>` with a sub-struct holding the eleven fields. `next_turn` =
  `Some(TurnState::new(event_context, security_context, classification, publication))`;
  `commit_turn`/`abandon_turn` = `None` plus what they take over. A stored `Some` is a turn to
  resume after a crash; a deliberate stop abandons the turn before storing, so it leaves `None`.
- `processing_state` becomes a plain `ActorState` in the document; the actor owns its
  `ProcessingStateHolder` and writes the value in when it stores. `processing_state_serde` and
  `default_processing_state` go.
- `Turn` keeps its shape; its methods reach `self.state.turn` instead of eleven fields (~60
  accesses, all in `agent_turn.rs`). `channel_id()`/ `session_id()` come from the classification as
  today.
- Alternative considered: keep the fields, merge the three reset lists into one. Cheaper, but leaves
  "no turn" a convention. Not recommended.

## Done when

- `AgentChannelState` has one field for the turn's working state, and it is `None` between turns.
- No `Arc` or atomic is serialised.
- Loading a state with `turn: None` yields an idle actor; with `turn: Some` the actor resumes as 75
  specifies.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 3.1.3 and 3.1.6).
  Asked for explicitly by the maintainer.
- **2026-09-29** — Done, after three points were settled with the maintainer that the considerations
  above had not foreseen:
  - (A) `perceive` begins no turn: the perceive processors only append to the transcript and the
    history. Otherwise every perceived message would have left a `Some` in the store of an idle
    agent. This also does point 3 of [90](90-message-path-wake-and-queue.md).
  - (B) The last event of a turn is sent by the ended turn: `commit_turn` and `abandon_turn` return
    an `EndedTurn` (in `CommittedTurn::ended` for the former) that sends with the contexts and
    publication the turn had; `reflect` and `fail_turn` use it. On the maintainer's review,
    `commit_turn` takes the `TurnState` apart and uses every part once, and `EndedTurn` keeps only
    those three values - not a turn whose staged changes and knowledge had been taken out, which
    would have looked complete to a later reader.
  - (C) Between turns, reading the working state gives the values of an empty `TurnState`; changing
    it trips a `debug_assert!` in tests (and starts an empty one in a release build). Tests that
    work in a turn begin one (`Turn::begin`, test-only). `AgentChannelState` now holds
    `processing_state: ActorState` - plain; the actor writes its live value in when it stores - and
    `turn: Option<TurnState>` with the twelve former fields; `processing_state_serde` and
    `default_processing_state` are gone. `AgentActor::new` no longer presets turn fields on load;
    the agent's name for the `Turn` comes from its controller. `prepare_resume` reads the tool calls
    from the turn and starts a turn state without a turn as `Idle`. `Turn::take_requested_state` is
    gone. The assertion found a bug in this very change: `handle_reflecting` read the requested
    state after `commit_turn` had ended the turn, so `stop` and `bye` would have left the agent idle
    instead of asleep, with an empty turn stored. It reads it before; the test
    `an_agent_that_calls_stop_ends_its_turn_with_the_call_and_sleeps` (the test channel now has an
    approver) covers it. Noticed, not changed: the run loop stores the state the finished step *was*
    in, not the one it goes to. After a `Planning` step that produced tool calls, a crash resumes
    `Planning` and asks the model again with its own tool-call message already in the context.
- **2026-09-29** — The finding above, fixed at the maintainer's request as part of this issue: the
  run loop now sets the next state first and then stores, so the store holds the step that comes
  next. Resuming then does what the state says - `Planning` asks the model with everything the turn
  has so far, tool results included; `ExecutingTools` means the tools of that step were about to
  run, and calls without a result are answered as not executed (75); `Reflecting` takes the finished
  answer over. One consequence: a call approved and not yet run when the server stops is answered as
  not executed after the restart, so the model decides again
  - the approval is not replayed. Test: an agent whose model called a tool and then never answers
    again is stored in `Planning`, with the tool's result in its turn (with the old order it was
    stored in `ExecutingTools`).
