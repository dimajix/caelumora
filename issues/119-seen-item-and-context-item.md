# 119 — What the model saw and what it was given are two types

**Status:** done
**Area:** Context & knowledge

## The problem

`ContextItem` (`caelumora-model/src/message.rs`) is what a turn gives its
model: the conversation - transcript entries with their digests - and the
prompt parts where the context provider placed them. `SeenItem`
(`caelumora-model/src/agent.rs`) is what `AgentChannelState::resolve` gives
back when a `ContextRef` is resolved later: what the model saw when it
produced a message. They are the same thing in two shapes:

| | `ContextItem` | `SeenItem` |
|---|---|---|
| message | `Conversation { message, digest }` | `Message(message)`, no digest |
| prompt part | `Prompt { role, text }`, no digest | `Prompt { digest, role, text }` |

## Why it matters

Issues [06](06-context-in-message-for-auditability.md) and
[47](47-prompt-parts-with-provenance.md) rest on one claim: what was sent
and what was recorded come from one source. Two types for it mean two
shapes to keep in step, and a test can only compare them field by field.
The prompt digest was computed from the text in two places (`set_context`,
`prompt_refs`) because `ContextItem` did not carry it.

## Considerations

- Merge into `ContextItem`: `Prompt` gains its `digest`, set by a
  constructor `ContextItem::prompt(role, text)`; `resolve` gives
  `Conversation` with the entry's digest, which it has at hand anyway.
  `SeenContext` stays as the pair of the reference and its items.
- The REST DTO `SeenItemResponse` stays as it is towards the web UI -
  model types are not serialised onto the wire (issue
  [42](42-export-import-chats.md)) - and is built from `ContextItem`.

## Done when

- There is one type for an item of a model's context, and resolving a
  reference gives back the items the turn gave the model.

## History

- **2026-09-29** — Raised by the maintainer and done the same day as
  proposed. `SeenItem` is gone; `SeenContext::items` are `ContextItem`s.
  `ContextItem::Prompt` carries the digest of its text, computed once by
  `ContextItem::prompt`, which the `StateContextProvider` uses; `set_context`
  and `prompt_refs` read it instead of computing it again. REST builds
  `SeenItemResponse` from `ContextItem`; the web UI sees no change. The
  test of what an agent saw now also checks that the context resolved for
  a message is exactly - by kind and digest - what the turn gave the
  model.
