# 117 — `Message::prev_id` is excluded from the digest yet still set

**Status:** open
**Area:** Context & knowledge

## The problem

`Message::prev_id` (`caelumora-model/src/message.rs:25`) is deliberately
left out of the message digest "because the chain expresses it": every
chain a message is appended to sets the predecessor itself
(`Chain::push_with`, `MessageEntry`). Yet `ollama.rs:377` and
`actor_impl/agent.rs:397` set `prev_id` when they create a message, so the
field carries a value that no chain uses and no digest covers.

## Why it matters

A field that is set but never meaningful is read by the next person as
meaningful. Issue [08](08-retry-and-edit-past-answers.md) decided to fork by
`MessageId`, precisely because `prev_id` is per chain and overwritten -
that decision is easier to keep if the field does not pretend otherwise.

## Considerations

Either remove the field (the chain's `last_message_id()` answers the
question) or remove the two setters and document that the field belongs to
the chain. The first is cleaner and is what 08 implies; check what the web
UI and the context panel read.

## Done when

`Message` has no predecessor field that the chains do not own, and
nothing sets one.

## History

- **2026-09-28** — Split off from [97](97-small-model-unifications.md)
  (structure review, 3.4 F).
