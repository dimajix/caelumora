# 92 — `NoteChain` and `MemoryChain` are the same type written twice

**Status:** open
**Area:** Context & knowledge

## The problem

`caelumora-model/src/note.rs:68-122` and `memory.rs:74-132` have the same
methods (`new`, `digest`, `entries`, `push`, `current`, `next_id`), the same
fold in `current()`, the same search in `next_id()`. They differ in the
extra fields of an entry (`transcript` vs. `origin`) and the digest
function. `NoteChange`/`MemoryChange` are the same enum twice.

## Why it matters

Issue [65](65-generic-component-state.md) wants the core to stop knowing
kinds of component state; a third chain (the next component) would be a
third copy. The review of the components (`work/2026-09-28-code-review.md`,
4.2.3) concluded that a generic *component* does not pay off - the generic
part belongs here, in the model.

## Considerations

- A generic `Ledger<X>`: entry = `Change { Add { id, text }, Remove { id } }`
  + extra `X` + `after` + `written_at` + digest; `NoteChain = Ledger<NoteExtra>`,
  `MemoryChain = Ledger<ChannelClassification>`. The digest schema names
  stay parameters. About 100 lines less in the model; `agent_notes.rs` and
  `agent_memory.rs` adjust variant names; the stored form changes (no
  legacy data to keep).
- This is the first step of 65 and should be recorded there: the direction
  is "the core does not know the kind", not another `enum` case in
  `AgentChannelChange`.
- Keep `Chain<E>` underneath as it is - issue
  [08](08-retry-and-edit-past-answers.md) turns it into a tree.

## Done when

One type holds a chain of changes; notes and memory are two
instantiations of it.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.2.1).
- **2026-09-30** — Taken into the epic of the review of the agent actor
  ([151](151-agent-refactoring-epic.md)), still with its architecture
  decision open. The review proposes enums per kind that implement a
  common `Change` trait (the added id, the fold) under one
  `Ledger<C, X>`, rather than one enum for both. [156](156-notes-and-memory-staged-scans.md)
  builds on the fold and should follow it.
