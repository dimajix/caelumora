# 606 — No notes belong to the channel

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** Every note in a channel belongs to the agent that wrote it and is read there only while that agent takes part, so a channel has no notes of its own - nothing that stays on its board when its author leaves, that users can write, or that another than its author can correct.

## The problem

Notes belong to the agent that wrote them and are published in a channel
([19](19-memory-tiers.md), principles 2 and 3): each agent keeps its chain in its own
`AgentChannelState`, and what a channel's board shows (`ChannelView::notes`) is put together from
the states of the agents taking part in it. Decided in
[581](581-the-notes-of-an-agent-that-left-fall-off-the-board.md): an agent that leaves, is removed
or deleted takes its notes along.

What that cannot give is notes that belong to the channel itself:

- what was agreed, decided or found in the channel stays on its board when the agent that noted it
  leaves - a sub-agent ended ([558](558-no-agent-hands-a-task-to-a-sub-agent.md)), an agent
  removed or replaced by another personality;
- users take part as agents do, but keep no `AgentChannelState`, and so note nothing;
- a note that is out of date is corrected or revoked by whoever takes part, not only by its author,
  who may be gone.

## Wanted

Channel notes as an extension of their own, in a crate of their own - a plugin that keeps and
decides, and a library with the component -, as agent memory is brought by `caelumora-memory`
(see [MEMORY.md](../docs/MEMORY.md#agent-memory)). The difference to the notes of today: the notes
belong to the channel, not to its participants. Whether they replace the agents' notes or are an
alternative to them, chosen per personality as memory is to private notes, is to be decided.

## Considerations

- **Where they live.** A document of the plugin's kind per channel, as memory keeps one per agent;
  it is deleted and restored with the channel and purged with its session, as a deleted channel
  is ([CHANNELS.md](../docs/CHANNELS.md)).
- **Who writes, who revokes.** Every participant, users and agents alike; or every participant
  writes and only the author or whoever may change the channel (`UpdateChannel`) revokes. Each
  entry keeps its author, as provenance, not as owner.
- **Staged, then committed.** An agent's changes are staged by its turn and committed when the
  turn completes (`KernelBuilder::commits`), as memory's; a user's are written at once.
- **Confidentiality.** Every entry carries the label of the turn or the message it was written by
  and is read only where that may be read, as the notes of today are released entry by entry.
- **Branches.** The notes of today branch with the message tree
  ([19](19-memory-tiers.md), principle 2): a regenerated answer does not see what the abandoned
  one noted. Whether channel notes do the same, or stand for the channel as a whole, is to be
  decided; a turn given up commits nothing either way.
- **Concurrent writers.** Several agents and users write one chain; numbers are reserved by the
  plugin, as memory's are, so that two turns do not take the same number.
- **Where they are read.** A prompt part of the component, `read_notes` or a tool of its own, the
  web UI's "Notes" tab and `/channel notes` - beside the agents' notes or instead of them.

## Done when

- How channel notes relate to the agents' notes - instead of them, or chosen per personality - is
  decided, and who writes and revokes them.
- A crate brings them: the plugin with its kind and its extensions, the library with the
  component, documented in MEMORY.md and PLUGINS.md.

## History

**2026-11-25** - Opened while deciding
[581](581-the-notes-of-an-agent-that-left-fall-off-the-board.md): the maintainer decided that an
agent's notes go with it, and asked for notes that belong to the channel, ideally in a crate of
their own.
