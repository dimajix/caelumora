# 50 — `TurnContext` calls the model input "context"

**Status:** open
**Area:** Context & knowledge

## The problem

In `TurnContext` (`caelumora-runtime/src/actor.rs`) and `AgentChannelState`,
"context" means two different things side by side:

- `state_context()`, `stored_context()`, `extend_state_context()` and
  `AgentChannelState::context` are the stored *context*: the messages the
  agent's model is given as the conversation (`MessageContext`).
- `context()`, `full_context()`, `set_context()` and the field
  `AgentChannelState::turn_context` are what one call of the model is given:
  prompt parts ([47](47-prompt-parts-with-provenance.md)), the stored context,
  and (for `full_context`) the messages of the turn so far.

The terms decided in [19](19-memory-tiers.md) call the second one the *model
input* and reserve "context" for the first. The code does not follow yet.

## Why it matters

Every reader of the agent loop has to find out from the code which of the two
a `context` is. With notes ([17](17-information-sharing-within-session.md))
and branching ([08](08-retry-and-edit-past-answers.md)) both will be touched
more often, and the confusion will cost more.

## Considerations

- `input` / `model_input()` was proposed and not liked: too generic. Other
  candidates, each with a drawback:
  - `prompt` - collides with *prompt part* and the system prompt.
  - `request` - suggests the HTTP request to the LLM backend, which also
    carries tools and options.
  - `conversation` - is only the stored part, without prompt parts.
  - `model_messages` / `llm_messages` - accurate, but clumsy.
- Whatever the name, it should be one word for "what the model is given in
  one call", used for the field, the setter and the full sequence including
  the turn's messages, and match the term in [19](19-memory-tiers.md).
- `TurnContext` itself is an operation context in the sense of
  [19](19-memory-tiers.md) (like `SecurityContext`, `EventContext`) and is
  not meant to be renamed here.
- A pure rename, no change in behaviour.

## Done when

- A name is chosen and recorded here and in [19](19-memory-tiers.md).
- `TurnContext` and `AgentChannelState` use "context" only for the stored
  context.

## History

- **2026-09-25** — Issue created while defining the terms in
  [19](19-memory-tiers.md); renaming to `input` was proposed and deferred,
  since the name did not convince.
- **2026-09-28** — `TurnContext` (with `ContextItem`) moved from
  `caelumora-runtime/src/actor.rs` to its own module
  `caelumora-runtime/src/agent_turn.rs` at the maintainer's request;
  `actor.rs` keeps the `Actor` trait and `MessageContextError`.
