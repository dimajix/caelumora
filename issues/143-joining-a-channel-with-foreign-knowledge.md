# 143 — Whoever joins a channel later sees what it read from other sessions

**Status:** open
**Area:** Sessions and channels

## The problem

Whether a session may read what was said in another is decided by the
source session's privacy policy ([134](134-session-privacy.md)), and it is
decided at the time of reading, for the participants the reading session
has then. What the reading channel's agents produce from it stays in the
channel's history. A participant added afterwards sees that history -
including what came from a session whose participants never agreed to be
read by them.

`SharedAudience` makes this visible most plainly: it allows reading because
every participant of the reader also takes part in the source; one more
participant, and that no longer holds - but the knowledge is already there.

## Why it matters

- The consent a privacy policy stands for is given for an audience, and
  adding a participant widens the audience after the fact.
- It is the same kind of gap as [141](141-turn-during-reclassification.md):
  a decision taken once, whose premise changes later.

## Considerations

- **The channel knows where its knowledge came from.** Every message an
  agent produced records what the model was given, including the knowledge
  read elsewhere (`ContextRef::knowledge`, a list of `KnowledgeRef`s,
  issue 17). From it, the sessions a channel's history drew on can be
  derived - or kept as a summary in the channel's state, updated as
  messages are taken over.
- **The inviting user decides.** When a participant is to be added to a
  channel that holds knowledge from other sessions, the UI tells the
  inviting user which sessions it came from, and they decide, with all its
  consequences. The decision belongs in the audit log
  ([140](140-audit-log.md)).
- Whether adding is refused where the source's policy would not allow the
  new participant, or only warned about, is part of the decision.

What the `KnowledgeRef`s record today is not yet enough for this:

- A `KnowledgeRef` names a `ChannelId`, not the session of that channel.
- What an agent recalls from its memory is recorded as
  `KnowledgeSource::Memory` with the channel it was *read in* and the
  digest of the whole memory chain - not the channels and sessions the
  recalled entries came from (`MemoryEntry::origin`). That, of all things,
  is today the one way knowledge crosses sessions.
- Of a channel's history, the digest of the whole chain is recorded, even
  where only some entries were readable (a known gap of
  [133](133-classification-model.md)).

The first two are to be closed by [134](134-session-privacy.md), which opens
reading across sessions and should make it traceable from the start.

## Done when

- It can be told for a channel from which other sessions its history drew
  knowledge, including through an agent's memory.
- Adding a participant to such a channel shows the inviting user where the
  knowledge came from, and adds them only on their explicit decision.

## History

- **2026-09-30** — Opened in the discussion of
  [134](134-session-privacy.md): a privacy policy decides at the time of
  reading, and a participant added later sees what was read. The maintainer
  wants the channel to keep where its knowledge came from, and the UI to put
  the decision to the inviting user.
