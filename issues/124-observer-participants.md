# 124 — Nobody can join a channel just to watch it

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** Nobody can join a channel just to watch it: joining makes a participant who can write, and without joining it cannot be followed live.

## The problem

Following a channel live is for its participants (issue [104](104-sse-stream-and-subscribe.md)): the
live stream is the user's view through their `UserActor`. A user who wants to watch what happens in
a channel without taking part - two agents working something out between themselves, say - has no
way to: joining makes them a participant who can write, and without joining they can read the
history but not follow it.

## Why it matters

Channels of agents alone are a natural use of multi-agent sessions, and watching them is how their
owner learns what they do. Today the only way is to join as a full participant, whose presence the
agents see and to whose messages they respond.

## Considerations

- **An observer is a participant without the right to write.** They have a `UserActor`, so they
  watch through it like anyone else (the maintainer's direction: live only for participants, never a
  shortcut to the channel); what they may not do is send messages, approvals or commands. Where that
  is decided: a participant role on `Participant::User` / the channel (`observer: bool`, or a role
  enum), checked by the channel when it routes the actor's envelopes, or by the actor before it
  sends - the check belongs where the permission is (`Enforcement::require`, issue 95), e.g. an
  `ActionType::WriteChannel`.
- **Visible or not.** Does an observer appear among the members - to users, to the agents' context,
  to the moderator, to `@`-mentions? Probably visible to users and invisible to the moderator (never
  asked to respond, never mentioned). Completely invisible watching is the admin's, not an
  observer's (issue [125](125-invisible-admin.md)).
- **Joining and leaving** as an observer through the existing participant operations
  (`add_participant`, issue 46) with the role, and turning an observer into a writer and back.
- **History access** as for any joining participant (`HistoryAccess`).
- **Web UI**: a read-only chat view for observers - no input, a hint why.

## Done when

A user can join a channel as an observer, follows it live, and can neither write, approve nor run
commands in it.

## History

- **2026-09-29** — Opened by the maintainer while working on issue 104: live following became
  participants-only, and watching a channel of agents is the case that needs a read-only
  participant.
