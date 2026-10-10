# 303 — Notes and memory are shown as raw JSON

**Status:** done
**Area:** User interaction

## The problem

None of the tools that keep what an agent noted or remembers has a presentation of its own; all are
presented as every call (`ToolPresentation::Default`), their arguments and their result as the JSON
the tool returns:

- the notes of a channel (`NotesComponent`): `write_note`, `update_note`, `revoke_note`,
  `reclassify_note` and `read_notes`;
- private notes (`PrivateNotesComponent`): `write_private_note`, `update_private_note`,
  `revoke_private_note`, `reclassify_private_note`;
- the agent's memory (`AgentMemoryComponent`): `remember`, `update_memory`, `forget`,
  `reclassify_memory`.

So a note written shows `{ "text": "..." }` and `{ "status": "noted", "id": 3 }`, a note revoked
only `{ "id": 3 }` and `{ "status": "revoked", "id": 3 }` - which note that was, the chat does not
say. `read_notes` answers `{ "channels": [{ "channel", "notes": [{ "by", "text" }] }] }`.

## Why it matters

- Writing a note is one of the commonest things an agent does in a channel, and the notes tools
  are `SHOWN`: every member sees these calls, as JSON.
- The web UI already shows notes readably elsewhere - `NotesList` groups a channel's notes by
  agent, each with its text and its number, and `PrivateNotesPanel` does the same for private
  notes (issue [233](233-private-notes-panel.md)) - but not in the chat where they are made.
- The tools are shown unevenly next to the others: the file tools (issues 251, 265, 267) and the
  skill tools (issues [281](281-tool-argument-visibility.md), [301](301-skill-list-presentation.md))
  are shown as what they do.

## Considerations

The pattern is set: a tool declares its presentation, the web UI's `ToolCallView` has a handler for
each (`PRESENTATIONS`), and a client that does not know one falls back to the default.

- **How many variants.** The three kinds share their shape: one entry by number, written, updated,
  revoked/forgotten or reclassified; they differ in who may see it (channel, the agent alone in a
  channel, the agent across channels). Proposed: one variant for an entry acted on,
  `ToolPresentation::Note` (`note`), read from fields rather than tool names as `skill` is - the
  action from the result's `status`, the kind from a field the handler can label ("Note", "Private
  note", "Memory") - and one for the listing, `ToolPresentation::NoteList` (`note_list`), named
  after `skill_list` and `file_list`. A variant per kind would make the handler tell apart by
  variant what it shows the same way. *To decide.*
- **Which note an id names.** `update_*`, `revoke_*`/`forget` and `reclassify_*` take only an id;
  the chat cannot say which note was revoked, nor what an update replaced. Options: the result
  carries the entry's text (for an update, also the text before), so the handler shows it; or the
  handler shows only the number. The first costs tokens in what the model is given - the text it
  just wrote or already holds -, the second leaves the call hard to follow. *To decide.*
- **What the handler shows:** the action and the number, the text as text (its lines kept, as
  `NotesList` shows it); for a reclassification the new classification; for `read_notes` the notes
  grouped by channel and author, as `NotesList` groups them, and "Nothing noted" where there is
  nothing.
- **Visibility stays as it is.** The private-note and memory tools are `HIDDEN`: only the owner sees
  them, and the presentation changes nothing about that; where the frame says arguments or result
  are not shown, the handler shows what it has. A failed call shows its message.
- **Beside it, not part of it:** `read_channel` (`ChannelHistoryComponent`) is presented as default
  too; it reads messages, not notes, and would be an issue of its own.
- A test that each of these tools declares its presentation, as the one for the skill tools does.

## Done when

The notes, private-note and memory tools have a presentation (as decided), the web UI shows a note
or memory entry acted on as what was done to which entry, and a notes listing as the notes,
grouped; `TOOLS-AND-COMMANDS.md`, `WEB-UI.md` and `MEMORY.md` say so.

## History

- **2026-10-10** — Issue created at the maintainer's request: none of the notes, private-note and
  memory tools has a presentation of its own. Proposal above; how many variants, and whether the
  results carry the text of the entry they name, are still to decide.
- **2026-10-10** — Decided: two variants, `ToolPresentation::Note` (`note`) for an entry acted on
  and `ToolPresentation::NoteList` (`note_list`) for `read_notes`; the text shown is the new one
  alone, as the arguments of a note written or updated hold it - a note revoked, an entry forgotten
  or reclassified is named by its number, and no result carries a text. No field names the kind:
  the frame shows the tool's name already. Done: the notes, private-note and memory tools are
  presented as `note`, `read_notes` as `note_list`; the web UI's `NoteToolCallContent` shows the
  entry's number, what was done to it (the result's `status`), its new text and, for a
  reclassification, its new classification; `NoteListToolCallContent` shows the notes read, by
  channel and by author, numbered in the reader's channel - "Nothing noted" for none, "Notes not
  shown" where the result is the owner's. Tests: each component's tools declare their
  presentation. TOOLS-AND-COMMANDS.md, WEB-UI.md, MEMORY.md. Nothing stored changes.
