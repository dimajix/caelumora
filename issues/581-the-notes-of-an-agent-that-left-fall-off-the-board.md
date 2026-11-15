# 581 — The notes of an agent that left a channel are no longer read there

**Status:** open
**Type:** question
**Priority:** medium
**Area:** Context and knowledge
**Summary:** What is noted in a channel is read from the agents taking part in it alone, so once an agent leaves or is removed, its notes vanish from the channel - from the other agents' prompts, `read_notes`, the web UI and `/channel notes` - though its state, and with it the notes, is kept; nothing says whether that is meant.

## The problem

Notes belong to the agent that wrote them and are published in a channel
([19](19-memory-tiers.md), principle 2); what is noted in a channel is read as the channel's, as a
notice board is ([574](574-notes-and-memory-cannot-be-read-by-a-command.md),
[MEMORY.md](../docs/MEMORY.md#notes)). `ChannelView::notes` reads them from the states of the
channel's participants (`ChannelView::participants`): those taking part now, without the former
participants and the deleted agents resting in the channel.

An agent that leaves the channel (`/channel leave`), is removed from it (`/participant remove`) or
is deleted takes its notes off the board at once, although its state - the notes in it - is kept
"for the record and for a later return" ([CHANNELS.md](../docs/CHANNELS.md)). From then on they
are gone

- from the prompt part of the `notes` component of every other agent of the channel, and from
  `read_notes` in other channels,
- from the web UI's "Notes" tab (`GET …/channels/{cid}/notes`),
- from `/channel notes`,

and they come back if the agent returns. An agent whose personality was deleted still counts (a
test in `view/mod.rs` holds that); a deleted agent resting in the channel does not.

What other agents knew from those notes, and what they built on, is no longer what they are given;
the history still holds the messages the notes were written after. Nothing in the documents or the
issues says whether this is meant.

## Considerations

- **The board keeps what was pinned.** `ChannelView::notes` reads the former participants' states
  too, as the history keeps their messages; a note stays on the board until its author revokes it.
  Who left can no longer revoke it there - only by returning. Whether a purged agent's notes go
  with its state, as now, or are kept, belongs to the same decision.
- **Notes go with their author.** Notes are what an agent keeps public while it takes part; who
  leaves takes them along. Then it is documented in MEMORY.md and CHANNELS.md and held by a test.
- **Marked as former.** Read, but told apart - in the prompt part and the web UI - as notes of an
  agent that no longer takes part, so that other agents weigh them so.
- Either way, how a channel read entry by entry is answered (`nothing_released`) must count the
  same notes.

## Done when

- Whether the notes of a former participant are read in its channel is decided and documented in
  MEMORY.md.
- A test holds it, for an agent that left and for one deleted.

## History

**2026-11-15** - Opened from a finding while working on
[574](574-notes-and-memory-cannot-be-read-by-a-command.md): `ChannelView::notes` reads the
participants taking part alone.
