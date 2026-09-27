# 63 — Earlier reasoning is always given back to the model

**Status:** open
**Area:** Context & knowledge

## The problem

The Ollama adapter turns every message of the context into a chat message
and passes `Message::reasoning` along as `thinking`
(`caelumora-runtime/src/llm_impl/ollama.rs`). Every model call therefore
receives the reasoning of all earlier messages - the agent's own from
earlier turns, and, since the moderator's `remove_tools` keeps it, the
reasoning of other agents as well.

The documentation of several reasoning models says the opposite: earlier
reasoning is not to be given back in later calls; the chat templates of
some drop it, others do not, and some models degrade or change behaviour
when it is present. Within one turn, others require the reasoning that led
to a tool call to be passed back with that call (e.g. interleaved thinking
around tool use), but not the reasoning of earlier turns.

## Why it matters

- Tokens: reasoning is often longer than the answer, and repeating all of
  it in every call makes contexts grow fastest exactly where it is least
  wanted ([16](16-context-compression.md)).
- Quality: models used against their documented input format answer worse,
  and differently per model.
- Other agents' reasoning in a model's input is their inner view, which
  [61](61-reasoning-leaves-the-agent.md) is about to decide who may see at
  all.

## Considerations

- **Storing and passing on are separate questions.** The reasoning stays in
  the context for audit (the context panel, [41](41-show-message-context.md));
  what is decided here is only what goes into the next model call.
- **Rules that occur:** never pass reasoning back; pass back only the
  reasoning of the current turn (or only that of messages with tool calls in
  the current turn); always pass it back. Which applies is a property of the
  model, not of the agent.
- **Where the rule lives:** in the LLM adapter, per configured model - a
  setting of the model/LLM configuration (`reasoning_history: none |
  current_turn | all`) with a safe default (`current_turn` or `none`).
  Relates to model-specific behaviour in general
  ([57](57-prompt-catalogue.md), [49](49-plan-step-system-messages.md)).
- **Other agents' reasoning** should not reach a model at all, whatever the
  setting; that follows from [61](61-reasoning-leaves-the-agent.md) (the
  published form of a message) or is enforced where messages are routed to
  agents.
- The recorded context (issue 06) must record what the model was actually
  given, so if reasoning is dropped from the input, the digest and the
  context panel must reflect that - or record the rule that was applied.

## Done when

- Whether earlier reasoning is given to the model is decided per model by
  configuration, with a default matching what reasoning models document.
- No model receives another agent's reasoning.

## History

- **2026-09-27** — Issue created at the maintainer's request while
  discussing [61](61-reasoning-leaves-the-agent.md): some models must not
  see their reasoning in later calls, according to their documentation.
  Found while writing it up: the Ollama adapter passes every message's
  reasoning back, other agents' included.
