# 575 — An agent is not told the names of its session and channel

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** The prompt part `channel_briefing` tells an agent what its session and channel are for - their summaries and briefings - but not what they are called, and no other prompt part names them, so an agent cannot refer to the channel it talks in by its name, nor tell it apart from the others of its session.

## The problem

`channel_briefing` (`caelumora-agents/src/prompt/channel_briefing.rs`, text
`agents.channel_briefing`) renders, for the session and then the channel, "[About this session]
/ [About this channel]" with the summary and the briefing their owner wrote - and nothing where
neither says anything. The names are given nowhere: no prompt part of `caelumora-agents` renders
the session's or the channel's name; only `private_notes` names the channel a note was written in.

An agent therefore does not know which channel it talks in - asked about it, or about the other
channels of its session (`read_channel`, issue 264), it has nothing to name it by.

## Considerations

*   **Where.** In `channel_briefing`, which describes the places anyway - the name always, even
    where neither summary nor briefing says anything, so the part no longer comes empty -; or in a
    part of its own. A name is public for a session, as its descriptions are; a channel's is part
    of the channel, classified as the channel.
*   **Changes.** A channel renamed is told with the next turn, as a changed briefing is. An agent
    that keeps what it was told - an external one - is told the new name.
*   Issue [577](577-an-agent-is-not-told-where-it-runs.md) asks for the ids of session and channel
    beside the names.

## Done when

- An agent is told the names of its session and its channel.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.

**2026-11-14** - Done in `channel_briefing`, which now names every place by its name - the
session's read through `SessionReader::name`, added for it -, and adds what it is for where its
owner wrote it. The part is classified as the channel always, since the channel's name is part of
the channel; and since it holds something whenever there is a channel, it has no withdrawal any
more. The ids (issue [577](577-an-agent-is-not-told-where-it-runs.md)) are not given: what they
are for, searching the log, is still open there. Before it, `SessionView::name` answers a `Name`,
as `ChannelView::name` does.
