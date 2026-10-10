# 302 — A note cannot be changed, only revoked and written again

**Status:** done
**Area:** Context & knowledge

## The problem

The `notes` component (issue [17](17-information-sharing-within-session.md)) gives an agent
`note`, `revoke_note`, `read_notes` and `reclassify_note`. A note that is no longer quite right
cannot be changed: `note`'s description tells the model "To change a note, revoke it and note the
new version". That takes two calls, and the note comes back under a new number - numbers are never
given again (`AgentNotes`, [MEMORY.md](../docs/MEMORY.md#notes)) - so whatever referred to the old
number, in the conversation or in other notes, now points at a revoked one.

The names are uneven as well: `note` is a noun used as a verb, beside `revoke_note`, `read_notes`
and `reclassify_note`, which each name what is done to a note. The private notes of issue
[51](51-agent-private-notes.md) have the same gap and the same unevenness: `note_private`,
`revoke_private_note`, `reclassify_private_note`.

Raised by the maintainer.

## Why it matters

- Keeping notes up to date is what makes them worth reading; an agent that has to revoke and write
  again does it less often, and leaves stale notes standing - or writes a second note beside the
  first.
- A number that stays is what lets the agent, the other agents and the users follow one note
  through its changes; a revoke and a new note look like two unrelated notes.
- The same concept should have the same name everywhere (AGENTS.md): `write_note` beside
  `update_note`, `revoke_note`, `read_notes` reads as one family.

## Considerations

- **A change of its own in the ledger: `Change::Update { id, text, classification }`.** Notes,
  private notes and memory are one chain (`caelumora_model::ledger::Ledger<K>`, issue
  [92](92-ledger-for-notes-and-memory.md)), whose changes are `Add`, `Remove` and `Reclassify`.
  `Update` replaces the text of the entry `id` in the fold. Doing it as `Remove` plus `Add` with
  the same id was considered and not chosen: it would bend the rule that a number is given once,
  and the chain would no longer say that the note was changed rather than replaced.
- **Its classification.** The new text carries the turn's label, as a new note does (issues 133,
  168) - what the turn has seen may be in it. A release by `reclassify_note` applied to the old
  text and does not carry over; a lower classification for the new text needs a release of its
  own. Proposed so; *to decide*.
- **Where it stands.** An item records the message it follows and its origin. Proposed: an update
  moves both to where it was made, as the text is from there, and `written_at` with them. The
  alternative - keeping where the note was first written - says when the note began, not when what
  it says was written.
- **Same rules as `revoke_note`.** Only the agent's own notes, only notes of the turn's channel (a
  number names a note in one chain, [60](60-note-ids-in-prompts.md)), staged in the turn and taken
  over when it completes (`AgentNotes`, `NoteChange`); a note written or revoked in the same turn
  is seen as it stands with the turn's changes (`AgentNotes::effective`). No approval, like
  `note`: a note is shown to every member anyway (TOOLS-AND-COMMANDS.md, on what needs none).
- **The rename: `note` → `write_note`.** The tool's description, the prompt part's title ("change
  them with `note` and `revoke_note`") and the documents follow. For private notes, `note_private`
  → `write_private_note` and a new `update_private_note`, so that both families stay alike.
- **Memory.** `remember` and `forget` (`agent_memory`) are named as verbs of their own and need no
  rename. `update_memory` would come with the same `Change::Update` almost for free; whether it is
  wanted here or later is *to decide*.
- **Stored data.** `Change` gains a variant; chains already stored read as before. The rename
  changes no stored form, but stored transcripts and recorded contexts hold calls of `note` and
  `note_private`, which the model would then see as calls of tools it no longer has. Following
  AGENTS.md, no compatibility: the summary names the agent states and histories to delete. Rules
  in a configuration that name `note` (`ToolMatcher`) must be renamed by hand.
- **Related:** [58](58-notes-in-every-prompt.md) - the agent sees its notes a second time as tool
  calls; an updated note stands in the prompt part in its new form, while the call with the old
  text stays in the context, as a revoked note's does today. [55](55-notes-by-reflection.md) -
  tidying notes up would use `update_note`.

## Decision

By the maintainer (2026-10-10): the proposals above as they stand, and `update_memory` with them.

1. **`Change::Update`** in the ledger; the entry keeps its number and its place among the others.
2. **The new text carries the turn's label**, as a new entry does; a release of the old text does
   not carry over.
3. **It stands where it was updated:** origin, the message it follows and `written_at` are the
   update's. For memory that means the entry is recalled from then on where one remembered in the
   updating turn's channel would be.
4. **`write_note`, `update_note`; `write_private_note`, `update_private_note`; `update_memory`.**
   `remember` and `forget` keep their names.

## Done when

- An agent changes the text of one of its notes with `update_note`, and of a private note with
  `update_private_note`; the note keeps its number, and readers - the prompt parts, `read_notes`,
  the notes panels - see the new text.
- `note` is called `write_note`, `note_private` `write_private_note`; no description or prompt
  part tells the model to revoke and write again.
- Tests cover the update in the ledger's fold, within a turn and after it, of a note of another
  channel and of another agent (refused); MEMORY.md and TOOLS-AND-COMMANDS.md describe the tools.
  The summary names the stored data to delete.

## History

- **2026-10-10** — Issue created at the maintainer's request: an `update_note` tool, and `note`
  renamed to `write_note`. Proposal above; the classification of an updated note, where it stands,
  and whether memory gets `update_memory` are still to decide.
- **2026-10-10** — Decided as proposed, with `update_memory` (see *Decision*). Done:
  `Change::Update` in `caelumora_model::ledger`, folded in place with the update's classification,
  origin, message and time; `AgentNotes::update` and `AgentMemory::update`, which change only what
  `revoke` and `forget` may remove; the tools `update_note`, `update_private_note` and
  `update_memory`; `note` and `note_private` renamed to `write_note` and `write_private_note`. The
  tool descriptions and prompt parts no longer tell the model to revoke and write again. Stored
  agent states and histories that hold calls of `note` or `note_private` are to be deleted.
