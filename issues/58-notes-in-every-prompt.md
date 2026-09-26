# 58 — Notes are shown in full in every prompt and again as tool calls

**Status:** open
**Area:** Context & knowledge

## The problem

Notes ([17](17-information-sharing-within-session.md)) and private notes
([51](51-agent-private-notes.md)) reach the model as prompt parts before the
turn's input: every note of every agent of the channel, and every private
note the agent wrote in the other readable channels, in full, in every turn.
Two things follow:

- **Redundancy.** The agent that wrote a note also has the tool call
  (`note`, `note_private`) with its text, and its result, in its own
  context. It sees the note twice: once as the message in which it wrote it,
  once in the prompt part. Worse, a revoked note stays in the context as the
  old tool call: the prompt part no longer lists it, but the conversation
  still shows the agent noting it, and the model may keep relying on it.
- **Scaling.** The prompt parts grow with every note, across all agents of
  the channel and all channels of the session, and are paid for in every
  turn. Most of them are irrelevant to a given turn; beyond some number
  they cost tokens, dilute the model's attention and eventually crowd the
  context window ([16](16-context-compression.md)).

## Why it matters

- The note system is meant to become the session memory
  ([19](19-memory-tiers.md)); the more it is used, the worse the current
  approach gets - it penalises exactly the success it aims for.
- Stale knowledge from revoked notes undermines revoking, which is how
  "forgetting" is defined (principle 8 in [19](19-memory-tiers.md)).

## Considerations

- **Redundancy, options:**
  - Accept it: the tool call shows *when* and *why* the agent noted
    something, the prompt part what holds now. Cheap, but revoked notes
    linger.
  - Elide the note tool calls from the agent's own context once they are
    committed - replaced by a short placeholder ("noted #3"), so that call
    and result stay paired as providers require. The full call remains in
    the recorded context for audit. The prompt part is then the only place
    the text appears.
  - Leave it to context compression ([16](16-context-compression.md)),
    which will have to drop old messages anyway.
- **Scaling, options (not exclusive):**
  - A budget: the prompt part shows at most n notes or tokens, the newest
    or those of the own channel first, and says how many more there are
    (`read_notes` finds them).
  - Consolidation: fewer, better notes, written and tidied by reflection
    ([55](55-notes-by-reflection.md)).
  - Selection by relevance: only the notes that matter for the turn -
    retrieval ([59](59-retrieval-of-notes.md)).
  - Distinguish by source: the agent's own notes in this channel are few
    and always relevant; notes of other agents and private notes from
    other channels are the part that grows.
- Whatever is left out must still be recorded exactly: `KnowledgeRef`
  today records a chain's digest, which says "the whole chain was shown".
  Showing a subset needs the shown notes in the reference.

## Done when

- It is decided how the redundancy between note tool calls and prompt parts
  is handled, and revoked notes no longer linger in the agent's context as
  if valid.
- The notes given to a model no longer grow without bound with the number
  of notes in the session.

## History

- **2026-09-26** — Issue created at the maintainer's request, after
  reviewing the note implementation: the writing agent sees its notes twice,
  and putting all notes into every prompt scales badly.
