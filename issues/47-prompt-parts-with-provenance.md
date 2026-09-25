# 47 — Prompt parts are placed and recorded on two separate paths

**Status:** open
**Area:** Context & knowledge

## The problem

What an agent's model is given besides the conversation - today only the
system prompt - is handled twice:

- `TurnContext::set_system_prompt` records *what* was seen: the text by digest
  in `AgentChannelState.system_prompts`, the digest in `turn_system_prompt`,
  from where `record_turn` puts it into the `ContextRef` of every answer
  ([06](06-context-in-message-for-auditability.md)).
- `StateContextProvider` *also* places the system message itself, at the start
  of the context it passes to `TurnContext::set_context`.

What is placed and what is recorded only agree by coincidence. And the system
prompt is the only kind of prompt part there is, always at the start.

## Why it matters

- Notes ([17](17-information-sharing-within-session.md)) and other
  information that changes often (time, plan status, tools that could not be
  loaded, [32](32-unavailable-tools-invisible-to-model.md)) should not go into
  the system prompt. Many models put it first, so any change invalidates the
  whole prompt cache. Such parts belong near the end: placed transiently
  before the last new message of the turn, and never stored in the context.
  The stored conversation then stays a stable, cacheable prefix.
- A second kind of part at a second position makes the double path a real
  source of errors: what the model saw and what is recorded as seen would
  diverge.
- A transient part is in neither the context digest nor the system prompt
  digest. Without recording it, "what the agent saw" becomes incomplete.

## Considerations

The discussion on 2026-09-25 settled the division of responsibilities: the
`TurnContext` stays simple and does bookkeeping only; where parts go is the
`ContextProvider`'s logic.

- **One call, with a type that carries provenance.** Instead of
  `set_context(Vec<Message>)` plus `set_system_prompt`, the provider passes
  one sequence:
  ```rust
  pub enum ContextItem {
      /// An entry of the stored context, with its digest there.
      Conversation { message: Message, digest: Digest },
      /// A prompt part - the system prompt, notes, ... - not part of the
      /// conversation.
      Prompt(String),
  }
  ```
  `TurnContext::set_context(Vec<ContextItem>)` stores every prompt text by
  digest, records each part with an anchor (the digest of the conversation
  entry it follows, derived from the order the provider chose) and renders
  the messages for the turn, prompt parts as system messages. It knows
  nothing about positions or rules.
- **Recording:**
  ```rust
  /// A prompt part as seen: its text and the context entry it followed
  /// (`None`: before everything).
  pub struct PromptRef { pub text: Digest, pub after: Option<Digest> }
  pub struct ContextRef { pub context: Option<Digest>, pub prompts: Vec<PromptRef> }
  ```
  "After" rather than "before": every position except the very start has a
  predecessor, and "at the end" could not be expressed with "before".
  `AgentChannelState.system_prompts` becomes `prompts` (text by digest,
  independent of position), `turn_system_prompt` becomes the turn's
  `PromptRef`s.
- **Resolving.** `AgentChannelState::resolve` inserts the parts after their
  anchors into the context prefix and so reproduces exactly what the model
  was given, transient parts included. The position only matters when
  assembling; what is recorded is where a part actually was.
- **The provider side.** `SystemPromptProvider` becomes a `PromptProvider`
  whose parts carry a position, an enum in `component_impl::context`, not in
  the `TurnContext`: `Start` (the system prompt) and `BeforeTurnInput`
  (transiently before the last new message of the turn; at the end if the
  turn has no new message, e.g. after `/wakeup`). `StateContextProvider`
  collects the parts, takes the stored context entries *with* their digests
  (a new accessor; `MessageContext` has them, `state_context()` returns only
  messages) and places the parts. `ContextProvider`s run once, at the start of
  a turn, so a `BeforeTurnInput` part stays in the same place across the tool
  loop and stays cached within the turn.
- **Roles.** Parts at the start become the system prompt as today (Anthropic's
  `system` parameter). A part in the middle becomes a system message where the
  backend allows it, otherwise a clearly framed user message; that mapping
  belongs to each LLM backend.
- **Why a type and not "system messages are prompt parts".** Telling them
  apart by role would be fragile; a system message can be part of the
  conversation.
- **Rejected:** keeping `set_context(Vec<Message>)` and adding
  `record_prompt(text, after)`. Smaller, but keeps the double path.
- **Cost.** `TurnContext`, `StateContextProvider`, `SystemPromptProvider` and
  its implementations, `AgentChannelState` (fields and `resolve`),
  `ContextRef`, `record_turn`, the tests from 06 and 41; the REST response of
  the message context (today `system_prompt` and its digest) becomes a list
  of parts, and the web UI's context panel
  ([41](41-show-message-context.md)) shows them where they were. Stored data
  changes; legacy data need not be supported.

## Done when

- A `ContextProvider` passes the context as `Vec<ContextItem>`; the
  `TurnContext` records every prompt part with its anchor and places nothing
  itself.
- `SystemPromptProvider` is a `PromptProvider` with positions `Start` and
  `BeforeTurnInput`; a `BeforeTurnInput` part is transient and never stored in
  the context.
- `ContextRef` records all prompt parts; `resolve` reproduces the exact
  sequence the model was given; REST and the context panel show it.

## History

- **2026-09-25** — Issue created from the discussion about notes and prompt
  caching, as step 1 of phase 0 of the order of work recorded in
  [19](19-memory-tiers.md). Notes in phase 1 of
  [17](17-information-sharing-within-session.md) are a `BeforeTurnInput`
  part.
