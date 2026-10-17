# 06 — Persist the context a message was generated from

**Status:** done
**Area:** Audit
**Summary:** For a past message there is no way to reconstruct what the agent actually saw when it wrote it.

## The problem

An agent's reply is produced from a `MessageContext` assembled by the context providers for that
turn. The reply is persisted; the context it was built from is not — the context is mutable state
that the next turn overwrites.

So for any past message there is no way to answer "what did the agent actually see when it wrote
this?".

## Why it matters

This is the single most common question when an agent behaves oddly, and the one the system
currently cannot answer. It also matters for anything resembling an audit trail: an approval
decision made on the basis of a tool call is only reviewable if the state that produced the tool
call is recoverable.

The cost is that context is large and largely redundant between consecutive turns.

## Considerations

- Embedding the full context in every message is the simple answer and is probably too expensive at
  any real message volume — the context is dominated by history that is already stored once.
- A reference is cheaper than a copy: store a context *snapshot id* on the message and the snapshot
  separately, deduplicated. This interacts directly with [16](16-context-compression.md), which will
  make the context a derived artifact with its own lifecycle.
- Overlaps with [05](05-postgres-event-recorder.md): if the envelope stream is recorded, part of
  this is answered by replay rather than by storage. Decide which of the two carries the burden
  before building both.
- Whatever is stored inherits the sensitivity of the context — it may contain tool output and
  secrets-adjacent material.

## Done when

- For any persisted agent message, the context that produced it can be reconstructed.
- The storage cost of doing so is bounded and understood.

## History

- **2026-09-10** — Issue created from `TODO.md`, where it was phrased as *"Put 'context' into
  message for auditability?"* — the question mark is part of it: the design is not settled.
- **2026-09-25** — First step done, along the maintainer's design: the context is a hash chain, and
  the history refers to it.
  - **What was there:** `MessageContext` and `History` chained digests, but only over message
    ids (`id + previous digest`), so a digest named a sequence of ids, not a context - a message
    changed under the same id kept its digest. `HistoryEntry.context` existed but was never set: the
    history was filled only through `extend`, which wrote an empty string. The system prompt,
    assembled per turn by `StateContextProvider`, was neither hashed nor stored. Digests were hex
    `String`s, with the empty string for "none".
  - **Now:** a `Digest` type (`[u8; 32]`, hex in JSON). A message's digest covers its content - id,
    role, sender, content, reasoning, tool calls and results, `internal` - and the previous digest,
    fed unambiguously with length prefixes behind a version tag (`digest::message_chain`); JSON
    values are hashed with sorted keys, independent of `serde_json` features. Timestamps, usage
    statistics, mentions and metadata are left out: they are not what the model saw.
    `HistoryEntry.context` is an `Option<ContextRef>`: the digest of the context before the message
    and of the turn's system prompt. Chosen for the prompt: stored separately, deduplicated by its
    digest (`AgentChannelState::system_prompts`), rather than as entries of the context chain, which
    would have mixed messages and prompt in one chain. `TurnContext::record_turn` sets both when a
    turn ends; `AgentChannelState::seen_context_of` resolves them.
  - **Digests are stored and never recomputed.** A digest is the record of what was sent to the
    model, not a value to derive again: if the scheme changes (a `v2` of `message_chain`),
    recomputing on load would silently change every digest the history refers to, while stored
    digests let old entries keep theirs. The existing local state was developer test data only and
    was deleted, so no stored format had to be migrated.
  - **Storage cost:** the context is stored once per agent and channel, as before; a reference is
    two digests per message; each distinct system prompt is stored once.

  Still open: nothing outside the model reads the references yet - no REST endpoint or UI to ask
  what an agent saw; the channel's history (the one the UI shows) has no references of its own, the
  agent's state has to be looked up by the message's sender; and [16](16-context-compression.md)
  must keep what earlier references point to, since this relies on the context only growing.
- **2026-09-25** — Requirements for compaction ([16](16-context-compression.md)), from the
  maintainer's design discussion:
  - The context digest of a message must name exactly the context that was sent to the model to
    produce it.
  - Compaction produces a new context C′ from the old context C. Entries written before keep
    referring to C; only messages produced afterwards refer to C′ - they really saw only C′. So C
    has to remain resolvable after compaction: an agent's state will hold several context
    generations, and resolving a reference has to search all of them (today's
    `MessageContext::prefix` searches one append-only list).
  - "C′ came from C" is a relation of its own (e.g. `derived_from`), not the predecessor link of the
    chain. It is not sent to the model and therefore not part of C′'s digest: two identical contexts
    get the same digest however they came about.
- **2026-09-25** — REST endpoint: `GET
  /sessions/{id}/channels/{cid}/actors/{aid}/messages/{mid}/context`, answering the system prompt
  and the messages the agent's model saw, with the two digests its history recorded.
  - **Addressed by message, through the actor that produced it.** Asking by context digest was
    considered - the UI would need the reference in the message itself, which it is not: it lives in
    the producing agent's history. The same message is in every participant's history, but only one
    agent produced it, so the operation belongs to the actor, not the channel:
    `Actor::message_context(caller, message_id)`, with a default answer `NotRecorded` that only
    `AgentActor` overrides. One method and one endpoint deliver context and system prompt together.
  - **Who may see it:** a new `ActionType::ReadAgentContext` with the agent's attributes; under the
    ownership policy the agent's owner alone. Channel contributors see the conversation, not the
    agent's system prompt, internal steps and tool results.
  - The agent reads its state from the `StateStore` (written after every step) because its running
    loop holds the in-memory state.
  - Errors: `403 context_forbidden`, `404 context_not_recorded`, `404 context_unavailable`, `404
    actor_not_found`; `MessageContextError` is shown to users through `user_message`.

  Still open: showing it in the web UI (a separate step), and [16](16-context-compression.md), which
  must keep earlier contexts resolvable.
- **2026-09-25** — Showing the context in the web UI is now its own issue,
  [41](41-show-message-context.md).
- **2026-09-25** — Done: for every message an agent produces, what its model saw can be
  reconstructed, over REST and in the web UI ([41](41-show-message-context.md)), at the storage cost
  recorded above. What compaction must preserve is recorded in [16](16-context-compression.md).

