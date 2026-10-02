# 160 — Dead surface, duplicates and stale comments around the agent actor

**Status:** done
**Area:** Maintenance

## The problem

Small things, without a structural change:

- **Dead or crooked surface** (callers outside tests): `Turn::transcript()` 0, `Turn::channel_id()`
  1 and `session_id()` 4 (both are `channel().…`), `Turn::messages()` clones a `Vec` for one caller;
  `Transcript::last_message_id` 0, `Transcript::extend` returns digests both callers ignore,
  `MessageHistory::entries` 0; `new()` on `Transcript`, `MessageHistory`, `NoteChain`, `MemoryChain`
  beside `Default`. (`Chain::children` and `leaf_from` have no caller yet but are built for
  [08](08-retry-and-edit-past-answers.md) and stay.)
- **Duplicates in the actor:** "store the state, then store the document" three times;
  `handle_cancelling` and `handle_reflecting` differ only in the requested state; `kind: String` is
  always `"agent"`, `id` and `agent_id` are one value; `AgentActor::next_turn` builds the context
  and shares its name with `Turn::next_turn`; `begin_turn` has one caller.
- **Stale:** `message.internal = internal_message` is set twice in `handle_llm_turn`;
  `store_channel_state` has two doc comments; the `TurnState` doc names `abandon_turn`; the field
  docs of `AgentChannelState::history` and `transcript` describe neither.

## Why it matters

Each is small; together they are the noise a reader of the turn has to step over.

## Considerations

- One pass, one commit, before the structural issues of [151](151-agent-refactoring-epic.md).

## Done when

- The items above are gone or fixed.

## History

- **2026-09-30** — From the review of the agent actor (`work/2026-09-30-code-review-agent.md`, H1,
  M1, T3, A8), epic [151](151-agent-refactoring-epic.md).
- **2026-09-30** — Done.
  - Gone: `Turn::transcript`, `channel_id`, `session_id` (callers use `channel()`),
    `Transcript::last_message_id`, `MessageHistory::entries`, and `new()` on `Transcript`,
    `MessageHistory`, `NoteChain` and `MemoryChain` (`Default` instead). `Transcript::extend`
    returns nothing; `Turn::messages` returns an iterator instead of a cloned `Vec`.
  - Actor: the fields `id` and `kind` are gone - `id()` is `ActorId::Agent(agent_id)`, `kind()` the
    constant. `begin_turn` is part of `request_response`, `AgentActor::next_turn` is
    `assemble_context`. `reflect`, `handle_cancelling` and `handle_reflecting` are one
    `end_turn(turn, cancelled, output)`; a cancelled turn still lands `Idle` whatever a tool
    requested. `settle` moves the actor to a state and stores, where the run loop did both by hand;
    the third place, after a step, keeps its own form because of the cancel rule, which
    [152](152-three-signals-end-a-turn.md) removes.
  - Stale: the second `internal` assignment, the doubled doc comment, `abandon_turn` in the
    `TurnState` doc and the field docs of `history` and `transcript` are fixed; AGENT-TURN.md names
    `request_response` where the tools are asked for.
