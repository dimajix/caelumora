# 398 — "Given" names four different things around the transcript

**Status:** done
**Area:** Code structure

Part of [391](391-history-and-transcript-review-epic.md).

## The problem

Around an agent's transcript the word *given* means four things:

- `TranscriptEntry::Given`, `Transcript::give`, `AgentChannelState::given`, `Turn::give`: a message
  the agent was given to read - a skill, a summary ([16](16-context-compression.md),
  [389](389-knowledge-read-in-the-transcript.md));
- `TranscriptStore::given(ids)`: which of some messages the transcript holds, on whichever branch,
  what `AgentContextView::in_transcript` and the REST field `in_transcript` answer with
  ([196](196-perceive-processors-write-state.md));
- `JsonFileStore::given_dir`, `append_given` and the directory `<agent>.given`: the prompt texts
  and tool sets kept once by digest ([383](383-prompt-texts-and-tool-sets-kept-per-turn.md)), which
  the code otherwise calls `Kept`;
- `TranscriptStore::named_by` and `load_named_by_turn`, which read those - named by what a
  `ContextRef` *names*, a fifth word for the same values.

The first is the one the documents use (AGENT-TURN.md, *A message given*). The other three were
named before it, each for itself.

## Why it matters

- Whoever reads `state.given(...)` next to `transcripts.given(...)` has to look both up to learn
  that one records what the agent read and the other answers which messages it heard.
- A directory called `given` beside a `transcript` suggests it holds what was given to read; it
  holds the prompt texts.

## Considerations

- `TranscriptStore::given` → `holds` (or `held`), as the doc comment already says: "those of the
  messages the transcript holds". `AgentContextView::in_transcript` stays, it says what it says.
- `given_dir`, `append_given`, `<agent>.given` → `kept_dir`, `append_kept`, `<agent>.kept`, after
  the type `Kept` they store; STORAGE.md (*Prompt texts and tool sets*) renamed with them. Stored
  directories have to be deleted or renamed once.
- `named_by` and `load_named_by_turn` are about a `ContextRef` naming digests, which is the word
  used throughout (`ContextRef` *names* a transcript, prompts, tools); they can stay.
- Only renames; last in the order of the epic, after everything that would be renamed with it.

## Done when

- *Given* around the transcript means a message the agent was given to read, and nothing else.
- STORAGE.md names the directory of the prompt texts and tool sets as the store does.

## History

- **2026-10-16** — Opened from the review of the history and the transcript
  ([391](391-history-and-transcript-review-epic.md)).
- **2026-10-16** — Narrowed with the maintainer to `TranscriptStore::given`, and done there: it is
  `in_transcript` now, the name the view that asks it (`AgentContextView::in_transcript`) and the
  REST field it answers (`in_transcript`) have, so that the one concept has one name from the store
  to the client. Neither stored data nor the REST API changes.

  The problem above was put too strongly. *Given* means *given to the agent or its model* nearly
  everywhere: a message given to read, the message as the model is given it (`ModelProfile::given`,
  `ReasoningHistory::given`, `SenderFormat::given`, which the issue overlooked), and the prompt
  texts and tool sets the model was given, which `<agent>.given` holds, as its module says. So the
  directory and what hangs on its name - `given_dir`, `append_given`, the `MemoryStore`'s `given`,
  `given_store_behaves` - stay: renaming it to `.kept` would change where stored texts lie, leave
  the contexts of older messages unresolvable until stored data is deleted, and *kept* is the vaguer
  word of the two.
