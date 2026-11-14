# 573 — How full an agent's context is cannot be asked by a command

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The web UI shows under each agent a gauge of how full its context is, but no command tells it, so the terminal - which has no participant list - cannot show it at all, and nobody can ask before deciding on `/compact`.

## The problem

Every agent's state keeps what its calls used (`TokenUsage`, issue 209): the tokens its context
holds now (`context_tokens`, as the last call counted them or a compaction estimated them), the
size of its context (`context_size`) where known, and the totals of all its calls. The web UI
draws it as a gauge under each agent ([WEB-UI.md](../docs/WEB-UI.md), "Participants, moderator
and notes"), read through the participant's snapshot with `Detail::USAGE`.

No command tells it. The terminal has no participant list, so its user learns nothing of how full
an agent's context is - not even before deciding to run `/compact`, which exists as a command of
the agent.

## Considerations

*   **Which command.** A command of the channel (`/agent usage [agent]` or `/agent context
    [agent]`), answered from the participant's snapshot as the web UI reads it - for every agent
    of the channel without an argument -, or a command of the agent itself beside `/compact`
    (`/context @agent`). A command of the channel needs no agent to be awake and reads the state as
    stored; a command of the agent could tell what it holds right now.
*   **What it tells.** The tokens held against the size of the context, as a share where the size
    is known; the totals of the calls apart from it, as the web UI's hover card keeps them apart,
    since they grow much faster than the context.
*   **Who may ask.** Whoever reads the channel's participants sees the gauge in the web UI; the
    command answers the same, privately.
*   The terminal's full-screen status line (issue
    [572](572-the-full-screen-terminal-tells-nothing-of-other-channels-at-work.md)) could show it
    too, once a command or the snapshot gives it.

## Done when

- A command tells how full the context of an agent of the channel is, and what its calls used.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.
