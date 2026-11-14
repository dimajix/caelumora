# 574 — An agent's notes and memory cannot be read by a command

**Status:** open
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

## Done when

- A command shows the notes an agent published in the channel, and to its owner its private notes.
- A command shows its owner what an agent remembers, as decided.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.
