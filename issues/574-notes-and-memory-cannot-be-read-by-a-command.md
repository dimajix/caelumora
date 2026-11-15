# 574 — An agent's notes and memory cannot be read by a command

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** What an agent noted - its published notes, its private notes, its memory - is read only in the web UI's panels, and its memory only as the prompt part an agent was given, so the terminal cannot show any of it, and nobody can look up what an agent remembers without opening the context of one of its answers.

## The problem

An agent keeps three kinds of memory ([MEMORY.md](../docs/MEMORY.md)): notes it publishes in a
channel, its private notes, and its memory across channels and sessions (`agent_memory`, brought
by `caelumora-memory`). The views read the first two (`ChannelView::notes`, the participant's
`notes` and `private_notes`, `SessionView::private_notes_of`); the web UI shows them in its
"Notes" tab and its participant list. The memory is read by no view: its owner sees it only in
the context panel, as the prompt part an answer was given.

No command reads any of them, so the terminal shows none, and what an agent remembers right now
cannot be looked up at all.

## Considerations

*   **Which commands.** `/agent notes [agent]` for the notes published in the channel, and for its
    owner the private notes; `/agent memory <agent>` for its memory - a command the memory plugin
    contributes, as any component may contribute to a group ([COMMANDS.md](../docs/COMMANDS.md)),
    so that the kernel names nothing of a plugin's kind.
*   **Who may read.** Published notes whoever reads the channel; private notes and memory the
    agent's owner alone, as the views and the context panel let them today - asked of the policy,
    not by the command.
*   **What of the memory.** All of it, or what may be recalled in the channel asked from: the
    memory's entries carry the label of the turn that wrote them and the privacy of their session,
    and showing an entry where it may not be recalled would carry it past its rules
    ([CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md)). A reader of the plugin, in the owner's
    name, is missing either way.
*   **Recorded as read?** What an agent reads is recorded as knowledge; what a person reads by a
    command is not part of any turn.

Decided (2026-11-15), with the maintainer, in three steps. Notes stay the agent's that wrote them
([19](19-memory-tiers.md), principle 2), but what is noted in a channel is read as the channel's,
as a notice board is: every reader of the channel sees it, only who pinned a note takes it down.
What an agent keeps for itself - its private notes and its memory - is read by commands of the
agent, as its owner's.

1.  **`/channel notes [channel]`**, a command of the channel in the group `channel`, answered to
    the issuer alone: the notes the agents published in the channel, or in the one of the session
    named, by their authors, read through `ChannelView::notes`.
2.  **The private notes**, by a command of the agent that the component `private_notes` carries
    out, for the agent's owners alone (`owners_only`), read as its turn reads them.
3.  **The memory**, by `/memory @agent`, a command of the agent that the component `agent_memory`
    carries out, for the agent's owners alone: what of it may be recalled in the channel, as its
    `MemoryReader` gives it. A command of the channel that the memory's plugin contributes was
    weighed and set aside: the commands of a session are configured as components of the kernel's
    kinds, and the plugin would need a way of its own into them, and the kernel's helpers of the
    commands made public.

What a person reads by such a command is not recorded as knowledge: it is part of no turn's
context.

## Done when

- A command shows the notes an agent published in the channel, and to its owner its private notes.
- A command shows its owner what an agent remembers, as decided.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.

**2026-11-15** - Decided with the maintainer before the work began, in three steps: `/channel notes`
for what is noted in a channel, read as the channel's; commands of the agent for its private notes
and for its memory.

**2026-11-15** - Step 1 done: `/channel notes [channel]` (`command/channel/notes.rs`) reads
`ChannelView::notes`, of the channel issued in or the one named, which it finds as
`/channel describe` does (`command::channel::this_or_named`). A user reads a channel whole or not
at all, so the command meets no channel read entry by entry.
