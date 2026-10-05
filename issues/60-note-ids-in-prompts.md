# 60 — An agent cannot revoke every note of its own it is shown

**Status:** done
**Area:** Context & knowledge

## The problem

An agent revokes one of its notes by its id (`revoke_note`, `revoke_private_note`). Whether it has
that id depends on where the note is shown:

- Its own notes and private notes **in the current channel** are listed with their ids ("- #3: ...")
  in the prompt parts, and can be revoked.
- Its private notes **from other channels** are listed without ids ("From channel planning: - ...").
  They cannot be revoked from here, and nothing tells the agent so.
- Its own public notes in other channels appear in `read_notes` with an id, but that id is only
  valid in the other channel's chain; `revoke_note` revokes in the current channel only, so the id
  either fails or - since ids are small numbers per chain - hits a different note of the same number
  here.

## Why it matters

- An agent that recognises one of its notes as wrong, in whichever channel it sees it, should be
  able to correct it; otherwise wrong private knowledge keeps being recalled everywhere.
- An id that is ambiguous across chains can revoke the wrong note - an error that is silent.

## Considerations

- **Ids must say which chain they belong to** wherever notes of more than one chain are shown:
  qualified ids (`planning#3`) or ids unique per agent across channels. Qualified ids keep the small
  numbers the model handles well and make ambiguity impossible.
- **Revoking in another channel is writing into it.** A note chain lives in the agent's state for
  that channel, bound to that channel's branch ([08](08-retry-and-edit-past-answers.md), principle 1
  in [19](19-memory-tiers.md)) and written by that channel's actor. Options:
  - Not allowed: notes are revoked where they were written; the prompt part says so, and shows notes
    from elsewhere without ids. Simple and consistent with the ownership rules, but the agent cannot
    act on what it recognises as wrong until it is in that channel again.
  - Allowed, carried out by the agent's actor in the other channel (a message to it, applied at its
    next turn boundary), so that each chain keeps a single writer. Which branch the revocation
    belongs to is then the other channel's current one.
  - A local suppression: revoking a foreign note records, in the current channel's chain, that it is
    no longer to be recalled here. The origin is untouched; only this channel forgets. Cheap and
    branch-safe, but other channels keep recalling the note.
- **Retrieval** ([59](59-retrieval-of-notes.md)) will show subsets of notes from several chains;
  qualified, stable ids are needed there too.
- The tool descriptions and prompt headers must say truthfully which notes can be revoked.

## Done when

- Every note of its own an agent is shown carries an id that identifies it unambiguously, or is
  marked as not revocable from here.
- It is decided whether, and how, an agent revokes one of its notes from another channel.

## History

- **2026-09-26** — Issue created at the maintainer's request: notes of its own that appear in the
  prompt should have an id by which the agent can revoke them. Stated here as found: notes of the
  current channel already carry their ids; the gap is notes from other channels.
- **2026-10-05** — Done, deciding against revoking in another channel: notes are revoked and
  reclassified where they were written. A chain belongs to the agent's actor in its channel and to
  that channel's branch; writing into it from elsewhere would need a message to that actor or a
  local suppression - much machinery for a rare case, against the ownership rules. A wrong private
  note is recalled elsewhere until the agent is in its channel again; that is accepted.

  So the gap was not a missing way to revoke, but numbers where none is usable. The private notes
  of other channels were already unnumbered; their header now says they can be changed only there.
  `read_notes` lists a number only for notes of the turn's own channel (`listed_note`), and its
  description says so - before, a number from another channel's chain could silently revoke a
  different note of the same number here. Documented in MEMORY.md.

  Qualified ids (`planning#3`) are needed only once notes of several chains are shown side by side
  for the agent to act on; that is retrieval, [59](59-retrieval-of-notes.md), where it is noted.
