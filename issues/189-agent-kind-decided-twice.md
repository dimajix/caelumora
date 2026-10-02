# 189 — An agent's kind is decided in two places, by different rules

**Status:** open
**Area:** Code structure

## The problem

Whether an agent runs here or elsewhere (issue [179](179-external-agents-via-a2a.md)) follows from
its personality, and two places read it and `match` on `PersonalityKind`:

- `channel::Member::of` reads the personality in the agent's own context; a missing one is an error.
- `view::member::member_state` reads it as the system; a missing or unreadable one silently falls
  back to `Internal` - an external agent is then shown through `AgentChannelState`, a state it never
  writes.

`member_state` also reads the personality for every `MemberView` right away, though most callers
only need the name; before, it was read when the introduction was asked for. That adds to issue
[182](182-entity-reads-not-cached.md) on per-turn and per-request paths.

## Why it matters

A third kind, or a change in how the kind is decided, has to be made twice, and the channel and the
views can disagree about what an agent is.

## Considerations

- One owner resolves an agent to its kind - the `AgentController`, or a personality controller of
  issue 182 - and both callers use it, with one rule for a missing personality.
- Where the channel runs, the views could take the kind from the `ChannelLoop`'s `Member`, which it
  keeps per participant.
- Reading the personality lazily in `member_state`, as `UserMember` reads the profile, removes the
  extra read independently.

## Done when

- `PersonalityKind` is resolved for an agent in one place, and a member view reads no personality it
  does not use.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series.
