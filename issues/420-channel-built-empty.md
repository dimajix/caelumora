# 420 — A channel is built empty, and its participants join by the one way in

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Sessions and channels
**Summary:** A channel's founders are written into its document as it is built and never join, so no `ParticipantJoined` records them; build it empty and let its builder add them as anyone is added.

## The problem

There are two ways into a channel. `Channel::build` takes the founders' list and writes it into the
document; `SessionChannels::create_channel` hands it what the creator asked for. Everyone added
later goes through `ChannelController::add_participant`, and as their actor starts, `run` records
`HistoryShared` and `ParticipantJoined` in the adder's name (issues 408, 417).

The founders never join: nothing in the channel's history says they are members, nor since when.
Issue 417, step 7, reads membership from the chain alone - every member's `ParticipantJoined`,
the end of membership from its `ParticipantLeft` - and derives each member's visible ranges from
it. A founder would read nothing.

Two ways in are also two places where admission is checked, the group of the participants is kept
(`keep_participants`) and a newcomer is announced.

## Decision

To be discussed. The direction: `Channel::build` takes no participants; whoever builds a channel
adds them after, through the one way in, in the creator's name - so every member, founder or not,
joins by `add_participant`, is admitted by the same checks and is recorded by `run` the same way.

Open questions:

- **Before `run` runs.** A channel just created does not run yet; its founders are recorded as
  `run` starts their actors - or `add_participant` records the join itself when the channel does
  not run. Which one, so that the positions of the joins come before anything said.
- **How much history a founder is given.** Nothing was said yet: `from` is the start.
- **Restoring a channel** (issue 348) restores its participants with it; they were members
  before, their joins are in the restored chain.
- **The group of the participants** is then kept by `add_participant` alone.

## History

- 2026-10-18: Opened from issue 417, step 7, which needs every member's `ParticipantJoined`.
