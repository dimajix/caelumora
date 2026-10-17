# 70 — Channels are stored inside the session document

**Status:** done
**Area:** Sessions and channels
**Summary:** Channels are stored inside the session document, so every change to a channel has to go through the session and renaming a channel is not stored at all.

## The problem

A channel is not a document of its own: it lives in the session document (`Session.channels`), and
only the `SessionController` - with its write lock
- writes that document. A change to a channel therefore has to go through the session:

- `ChannelController::update_name` changes only the in-memory copy of the channel: nothing is stored
  (after a restart the old name is back), no permission is checked, and the `SessionController` has
  only a `TODO` for `ChannelUpdated`. Nothing calls it; renaming channels is an unfinished path.
- Adding and removing participants ([46](46-channel-participants.md)) would have to write the
  session document as well, so the `ChannelLoop` would need the `SessionController` - or the
  `ChannelController` would write the session document behind its back, racing the
  `SessionController` (the lost update of [62](62-json-store-concurrent-writes.md) in another
  place).
- Permissions on a channel are asked as permissions on the session (`UpdateSession`), although a
  channel has attributes of its own (owner, contributors derived from its participants,
  [11](11-agent-ownership.md)) and a confidentiality level ([61](61-reasoning-leaves-the-agent.md)).

## Why it matters

- A channel is becoming an object of its own: its own participants that change, its own level, its
  own name. One document, one writer is the rule the runtime follows since
  [62](62-json-store-concurrent-writes.md).
- [46](46-channel-participants.md) builds on it.

## Considerations

- **Channels become entities in the `EntityStore`**: `create_channel`, `get_channel`,
  `update_channel`, `delete_channel`, checked by the channel's own attributes (new `ActionType`s for
  channels). The session document keeps only the ids of its channels, in order (the order the web UI
  shows them in).
- **The `ChannelController` writes its channel**, like the `SessionController` its session: renaming
  (fixing the unfinished path) and, with [46](46-channel-participants.md), participants.
- **Creating and deleting a session** create and delete its channels; **loading** a session loads
  its channels. A running session loads them in the runtime's name (`SecurityContext::system()`):
  starting a session the caller may read follows from an action already permitted, and a session
  must not fail to start because the caller is not in one of its channels.
- **Creating and deleting a channel** ([09](09-creating-channels.md)) write both documents: the
  channel, and the session's list of ids.
- `SessionTemplate::instantiate` yields the session and its channels.
- REST and the global snapshot read the channels of a session from the store (or the running
  session) instead of the session document.
- No legacy data has to be read.

## Done when

- Every channel is a document of its own, written only by its `ChannelController` (and created or
  deleted with its session), with permissions checked on its own attributes.
- The session document holds only the ids of its channels.
- Renaming a channel is stored.

## History

- **2026-09-28** — Issue created at the maintainer's request while discussing
  [46](46-channel-participants.md): the `ChannelController` should manage its channel itself, as it
  already seemed to for renaming - which turned out to change only the copy in memory. Keeping
  channels in the session document and letting the `ChannelController` call the `SessionController`
  inside was considered and rejected as hiding the second writer rather than removing it. To be done
  before [46](46-channel-participants.md).
- **2026-09-28** — Implemented. The `EntityStore` has `create_channel`, `get_channel`,
  `update_channel` and `delete_channel` (memory and JSON stores, the JSON one under
  `runtime/channels/`), checked with the new `ActionType::GetChannel` / `CreateChannel` /
  `UpdateChannel` / `DeleteChannel` on the channel's own attributes - the ownership policy lets
  owners change and delete, contributors read. `Session.channels` holds `ChannelId`s;
  `SessionTemplate::instantiate` yields `(Session, Vec<Channel>)`; `SessionManager::create_session`
  stores the channels with the session (and takes everything back if one fails), `load_session`
  loads them in the runtime's name, `delete_session` deletes them. `SessionLoop::create_channel`
  stores the channel before listing it in the session, `delete_channel` deletes it after taking it
  out. The `ChannelController` writes its channel under a lock: `update_name(ctx, …)` stores first
  (`UpdateChannel`), then takes it over and announces it; it no longer applies `ChannelUpdated` from
  outside. The global snapshot reads the channels of a stopped session from the store, those the
  viewer may read. Tests: a created channel is stored as its own document; deleting a session
  deletes its channels; renaming is stored and refused to a contributor.
