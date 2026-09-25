# 49 — Plan step instructions are system messages in the middle of a turn

**Status:** open
**Area:** Tools & approvals

## The problem

While an agent executes a plan, `AgentActor::plan_step_message` and
`plan_finalize_message` (`caelumora-runtime/src/actor_impl/agent.rs`) push
the instruction for the next step as a message with `MessageRole::System`
into the middle of the turn.

Some models' chat templates merge every system message into the system
prompt and move it to the front. For them the instruction is not where it was
put, right before the step it is about, but part of the system prompt:

- its position, and so its meaning ("now do step 3"), is lost;
- the system prompt changes with every step, which invalidates the whole
  prompt cache ([47](47-prompt-parts-with-provenance.md)).

## Why it matters

Plan execution is the one place where the runtime deliberately steers the
model mid-turn. That it depends on the chat template whether the steering
lands where it was meant is fragile, and hard to see from outside.

## Considerations

- [47](47-prompt-parts-with-provenance.md) settled the same question for
  prompt parts: a part in the middle of the context is given as a user
  message, not a system message. The plan step instructions could follow the
  same rule, clearly framed as instructions of the runtime.
- The instructions are internal messages (`internal: true`) in the turn, not
  prompt parts: they are recorded in the context. Whether they should become
  prompt parts, or stay messages with another role, is part of the decision.
- Alternatively the role could be left to the LLM backend, which knows its
  model: a system message where the model keeps it in place, a user message
  otherwise. Today there is only the Ollama backend, which passes the role on
  unchanged.

## Done when

- Plan step and finalize instructions reach every model where they were
  placed, independently of how its chat template treats system messages.

## History

- **2026-09-25** — Issue created while implementing
  [47](47-prompt-parts-with-provenance.md), prompted by the observation
  that some models merge all system messages and handle them first.
