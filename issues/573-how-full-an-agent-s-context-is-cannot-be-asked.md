# 573 — How full an agent's context is cannot be asked by a command

**Status:** in progress
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

Decided (2026-11-14), with the maintainer, in two steps:

1.  **The command.** `/agent usage [agent]`, a command of the channel in the group `agent`,
    without a short form, answered to the issuer alone: it answers at once, also while the agent
    is in a turn or asleep, and enters no history. A command of the agent would wait for its turn
    and tell no more, since what was added since the last call is not in `context_tokens` either
    way. It reads the stored state through a method of its own, `ParticipantView::usage()` beside
    `ParticipantView::notes()`, and fails if the state cannot be read - not through the
    participant's snapshot, which asks every question behind its `can_…` fields and leaves out
    what it cannot read. The share of the context held is the domain's (`TokenUsage::filled`).
    A line each tells an agent's context, against its size where known, and the totals of its
    calls, input and output apart. Without an argument, every agent taking part is listed, and a
    line adds up their totals, as "Total tokens" does above the web UI's list - of the agents
    taking part only: what an agent removed or deleted used falls out of it, as out of the web
    UI's. No command of its own, `/channel stats`, for the sum: it would have nothing but the sum
    of what `/agent usage` lists.
2.  **The REST API and the web UI.** The web UI reads an agent's usage again after each of its
    turns, commands and compactions (`readUsage`) through
    `GET /sessions/{id}/channels/{cid}/participants/{aid}`, which answers the whole snapshot:
    every question behind the `can_…` fields is asked and its answer thrown away, and a failed
    read is answered as no usage. A route of its own, `…/participants/{aid}/usage`, answers it
    from `ParticipantView::usage()`, and `readUsage` reads it there; the profile panel keeps
    reading the whole participant. The stream's snapshot stays as it is: it needs the whole
    participant, reads the state once for all its details, and shows the channel without what of
    it cannot be read.

## Done when

- A command tells how full the context of an agent of the channel is, and what its calls used.
- The web UI reads an agent's usage by a route of its own, which answers a failed read as an error.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.

**2026-11-14** - Decided with the maintainer before the work began, in two steps: the command
`/agent usage`, read through `ParticipantView::usage()`; then a route of the REST API of its own
for an agent's usage, which the web UI reads after a turn instead of the whole participant.

**2026-11-14** - Step 1 done: `/agent usage [agent]` (`command/agent/usage.rs`) reads through
`ParticipantView::usage()`, the share of the context held is `TokenUsage::filled`. The commands
that seek among those taking part read them by one function, `command::taking_part`, and find
those that can be named by another, `command::named_participants`, which `/participant remove`
and `/channel leave` share with it.

**2026-11-15** - Step 2 done: `GET …/participants/{aid}/usage` (`get_participant_usage`) answers
`ParticipantView::usage()` - `null` where it is `None`, an error where the state cannot be read -,
and the web UI's `readUsage` reads it there (`api.getParticipantUsage`). Decided with the
maintainer after its review: `GET …/participants/{aid}` no longer answers the usage - it read the
agent's whole state, its transcript loaded with it, for what the profile panel does not show.
