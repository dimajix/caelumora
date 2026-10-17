# 59 — Relevant knowledge reaches the model only in full or by the model searching

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Context and knowledge
**Summary:** Knowledge reaches the model only in full or when the model itself searches for it; nothing selects what is relevant to the turn.

## The problem

What an agent knows beyond the conversation reaches its model in two ways today: in full, as prompt
parts listing all notes ([58](58-notes-in-every-prompt.md)), or when the model itself decides to
search, with `read_notes` and `read_history`. There is no way in between: a component that picks
what is relevant to the current turn and puts only that into the context - retrieval-augmented
generation (RAG).

## Why it matters

- Putting everything in does not scale ([58](58-notes-in-every-prompt.md)).
- Leaving the search to the model depends on it realising that it might know something - the same
  weakness as leaving note taking to it ([55](55-notes-by-reflection.md)). Knowledge it does not
  know it has is never looked up.
- Later sources - knowledge beyond the session ([18](18-knowledge-sharing.md)), documents, skills
  ([22](22-skill-management.md)) - are too large to be put in full at all; they will need retrieval
  anyway.

## Considerations

- **As a component on an existing extension point.** A retrieval component is a `PromptProvider`
  with a `BeforeTurnInput` part ([47](47-prompt-parts-with-provenance.md)): it gets the
  `TurnContext`, forms a query from the turn's input (the new messages, perhaps the last few),
  selects the relevant entries and renders them. The part is transient, so a selection changing
  every turn does not hurt the prompt cache. Retrieval by a component and search by tool are not
  exclusive; the tools stay for explicit lookups.
- **How to select**, options:
  - Lexical (BM25 / full text): no model needed, predictable, weak with paraphrases and across
    languages ([56](56-response-language.md)).
  - Embeddings and vector similarity: needs an embedding model (Ollama and the usual APIs offer
    them - an `LLM`-like provider abstraction), handles paraphrase and language, but adds a model
    call per note and per query.
  - Hybrid, or reranking the candidates by a small model.
  - A threshold as well as top-k, so that nothing is shown when nothing is relevant.
- **The index is derived data.** The note chains stay the source of truth; an index (embeddings,
  text index) is built from them and can be rebuilt. Where it lives: in memory per session, rebuilt
  at start (simple, fine for session-sized data), or persisted beside the `StateStore` once
  knowledge outlives sessions ([18](18-knowledge-sharing.md)).
- **Access rules apply before ranking.** Only notes of channels the turn's channel may read
  (`ReadChannel`, principle 11 in [19](19-memory-tiers.md)) and private notes within their
  confidentiality (principle 12) are candidates; the index must not become a way around them.
- **Only the active chain.** Revoked notes and, once branching exists
  ([08](08-retry-and-edit-past-answers.md)), notes of abandoned branches are not candidates; the
  index is keyed by note so that it can be filtered by the current chain state rather than rebuilt
  on every change.
- **Provenance becomes finer.** `KnowledgeRef` records a chain's digest, meaning the whole chain.
  With retrieval it must record which notes were shown (note ids with the chain digest), so the
  context panel ([41](41-show-message-context.md)) can show them and a later invalidation can tell
  whether a message depended on a note.
- **What stays always shown.** The agent's own notes in the current channel are few and always
  relevant, and it needs their ids to revoke them ([60](60-note-ids-in-prompts.md)); retrieval
  matters most for others' notes and for private notes from other channels. The histories of other
  channels are a further candidate source.

## Done when

- It is decided whether knowledge is selected by relevance for the turn, how, and for which sources.
- If so: a component puts the relevant notes into the context, respecting the access rules and
  recording exactly what it showed.

## History

- **2026-09-26** — Issue created at the maintainer's request: instead of putting all notes into the
  prompt, or letting the model search with a tool, a component could retrieve the relevant ones and
  put them into the context.
- **2026-10-05** — From [60](60-note-ids-in-prompts.md), done: notes are revoked where they were
  written, and a note is shown with its number only in its own channel. Retrieval that shows the
  agent its notes from several chains together, for it to revoke or reclassify, needs ids that say
  which chain they belong to (`planning#3`) - or shows the notes of other chains without numbers, as
  today.
