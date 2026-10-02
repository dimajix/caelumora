# 50 — `TurnContext` calls the model input "context"

**Status:** done
**Area:** Context & knowledge

## The problem

In `TurnContext` (`caelumora-runtime/src/actor.rs`) and `AgentChannelState`, "context" means two
different things side by side:

- `state_context()`, `stored_context()`, `extend_state_context()` and `AgentChannelState::context`
  are the stored *context*: the messages the agent's model is given as the conversation
  (`MessageContext`).
- `context()`, `full_context()`, `set_context()` and the field `AgentChannelState::turn_context` are
  what one call of the model is given: prompt parts ([47](47-prompt-parts-with-provenance.md)), the
  stored context, and (for `full_context`) the messages of the turn so far.

The terms decided in [19](19-memory-tiers.md) call the second one the *model input* and reserve
"context" for the first. The code does not follow yet.

## Why it matters

Every reader of the agent loop has to find out from the code which of the two a `context` is. With
notes ([17](17-information-sharing-within-session.md)) and branching
([08](08-retry-and-edit-past-answers.md)) both will be touched more often, and the confusion will
cost more.

## Considerations

- `input` / `model_input()` was proposed and not liked: too generic. Other candidates, each with a
  drawback:
  - `prompt` - collides with *prompt part* and the system prompt.
  - `request` - suggests the HTTP request to the LLM backend, which also carries tools and options.
  - `conversation` - is only the stored part, without prompt parts.
  - `model_messages` / `llm_messages` - accurate, but clumsy.
- Whatever the name, it should be one word for "what the model is given in one call", used for the
  field, the setter and the full sequence including the turn's messages, and match the term in
  [19](19-memory-tiers.md).
- `TurnContext` itself is an operation context in the sense of [19](19-memory-tiers.md) (like
  `SecurityContext`, `EventContext`) and is not meant to be renamed here.
- A pure rename, no change in behaviour.

## Done when

- A name is chosen and recorded here and in [19](19-memory-tiers.md).
- `TurnContext` and `AgentChannelState` use "context" only for the stored context.

## History

- **2026-09-25** — Issue created while defining the terms in [19](19-memory-tiers.md); renaming to
  `input` was proposed and deferred, since the name did not convince.
- **2026-09-28** — `TurnContext` (with `ContextItem`) moved from `caelumora-runtime/src/actor.rs` to
  its own module `caelumora-runtime/src/agent_turn.rs` at the maintainer's request; `actor.rs` keeps
  the `Actor` trait and `MessageContextError`.
- **2026-09-28** — Decided with the maintainer, differently from the issue's starting point.
  "Context" keeps its meaning in the LLM sense: what one call of the model is given, assembled on
  purpose - system prompt, notes, redacted parts, the conversation - so `context()`,
  `full_context()`, `set_context()`, `ContextItem` and `ContextProvider` are named right. The
  *stored* part is renamed instead, to **transcript**: the agent's own sequence of messages the
  context is built from - its messages in full, with tool calls and internal steps, and what others
  said as it received it; compressible later ([16](16-context-compression.md)). `view` was proposed
  and rejected ("context" is established for LLMs, and the provider does build a context);
  `conversation` was rejected because the context is not the conversation, and the history - the
  channel's public record - keeps its name. Other candidates weighed: `working_memory` (collides
  with agent memory), `dialogue` / `thread` / `record` / `log` / `trace` (too close to history or
  other meanings), `scratchpad` (within a turn). Renamed: `MessageContext` / `ContextEntry` to
  `Transcript` / `TranscriptEntry`, `AgentChannelState::context` to `transcript`, the accessors of
  the stored part, `ContextRef::context` to `transcript`, the component `state_context_appender` to
  `transcript_appender`. **`TurnContext` becomes `Turn`**, against the note above that it was not to
  be renamed: then "context" in its API means only the model's context (`turn.context()`), and a
  turn is a domain object of its own, not a helper for one call like `SecurityContext`. Two commits:
  the `Turn` rename, then the transcript.
- **2026-09-28** — Implemented. `TurnContext` is `Turn` (its variables `turn`); `MessageContext` /
  `ContextEntry` are `Transcript` / `TranscriptEntry`, `AgentChannelState::context` is `transcript`,
  the stored part's accessors are `transcript()`, `transcript_items()` and `extend_transcript()`,
  `ContextRef::context` and `NoteEntry::context` are `transcript`, the component
  `state_context_appender` is `transcript_appender` (`TRANSCRIPT_APPENDER_KIND`), and the REST field
  `context_digest` of the message context is `transcript_digest` (the web UI's context panel labels
  it "Transcript"). `context()`, `full_context()`, `set_context()`, `ContextItem`,
  `ContextProvider`, `ContextRef`, `SeenContext` and the message context endpoint keep their names:
  they are about the context. The terms in `ARCHITECTURE.md` follow. No change in behaviour; stored
  personalities with the old component kind have to be created again.
