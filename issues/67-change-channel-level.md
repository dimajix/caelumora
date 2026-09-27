# 67 — A channel's confidentiality level cannot be changed after creation

**Status:** open
**Area:** Sessions & channels

## The problem

A channel's confidentiality level ([61](61-reasoning-leaves-the-agent.md))
is set by its owner when the channel is created - in the session template or
the request - and cannot be changed afterwards. A channel created as
`session` that turns out to need `channel` (or the other way round) has to
be deleted and recreated, losing its history.

## Why it matters

- Confidentiality needs often become clear only during a conversation.
- The decision in [61](61-reasoning-leaves-the-agent.md) grants the owner
  the right to change it, lowering included for now.

## Considerations

- **Who:** the channel's owner only (`UpdateSession`, like creating and
  deleting channels, or a dedicated `ActionType`).
- **Where it is stored:** in the session document (`SessionController`,
  like `add_channel` / `remove_channel`) and in the running channel
  (`ChannelController`, like `ChannelUpdateEvent::NameChanged`), announced
  as `GlobalEvent::ChannelUpdated` so that open web UIs follow.
- **When it takes effect:** reading (rule 2) checks at every read, so it
  applies at once. Publishing is decided when a turn begins (`Publication`),
  so a running turn keeps the old decision until its end - acceptable, or
  the change waits for idle agents.
- **Raising** is harmless for what was published before: it was published
  under a less confidential level. **Lowering** releases the whole history:
  what was written under `channel` becomes readable from other channels.
  The decision allows it for now; the UI should say so before it is done.
  Whether to forbid lowering later, or allow it only for future messages,
  stays open.
- REST (`PATCH`/`PUT` on the channel) and the web UI (the owner changes it
  in the members panel, with the warning for lowering).

## Done when

- The owner of a channel can change its level, it takes effect for reading
  and publishing, and lowering is confirmed with a warning.

## History

- **2026-09-27** — Split off from [61](61-reasoning-leaves-the-agent.md) at
  the maintainer's request: part of its decision (the owner may change the
  level later, lowering included), not implemented with it.
