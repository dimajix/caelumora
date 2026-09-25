# 09 — Creating channels

**Status:** done
**Area:** Sessions & channels

## The problem

Channels can only come into existence by instantiating a `SessionTemplate`.
The REST API exposes `GET /sessions/{id}/channels`,
`GET /sessions/{id}/channels/{cid}` and the per-channel message, approval,
cancel and event routes — but no `POST`. There is no `create_channel` anywhere
in the runtime either; `SessionLoop` manages the channels it was built with.

So the set of channels in a session is fixed at creation time, and the only
way to get a differently-shaped conversation is to define another template and
start another session.

## Why it matters

A session is meant to be the container for an interaction context and a
channel one medium within it. Fixing the channels at template time collapses
that distinction: in practice a session *is* its channels. Anything that wants
a side conversation — a sub-task delegated to one agent, a private thread with
one participant — has nowhere to go.

## Considerations

- The lifecycle needs to be symmetric: creating a channel implies closing one.
  Both need `GlobalEvent`s, and `02faff5` already added the session/channel
  management events they would use.
- Participants are the substance of the question, not the channel itself. A
  new channel is only useful if agents and users can be placed into it, which
  means the `Participant` set has to be mutable at runtime and the
  `ChannelLoop` has to spawn actors on demand. `ApproverActor` already
  recomputes pending approvals on join/leave events, so part of the dynamic
  case is anticipated.
- Where does a dynamically created channel's `Moderator` come from — the
  request, the session template, a default?
- If an *agent* may create a channel (delegating a sub-task), this becomes a
  capability question and needs the security work in
  [12](12-user-and-agent-permissions.md).

## Done when

- Channels can be created in and removed from a running session via the
  client API and REST.
- Participants can be added to a channel after it exists.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-25** — Scope and approach decided:
  - **Split:** adding and removing participants moves to
    [46](46-channel-participants.md). This issue is about creating and
    deleting channels. The "done when" point on participants is covered there.
  - **Structure:** creating and deleting a channel changes the session
    document, like renaming it: stored first (where the permission is checked),
    then changed live, then announced. `SessionController` changes the
    document, and `SessionLoop` starts or stops the `ChannelLoop`. Every
    channel runs in its own task with an `AbortHandle` (the unused
    `ChannelHandle`), instead of a `JoinSet` filled once in `run()`.
  - **Building a channel:** one function builds a channel from participants,
    adding the approver and deriving the security attributes. It is used by
    both `ChannelTemplate::instantiate` and the new creation path.
  - **Participants of a new channel:** chosen from the session's participants.
    The creator is always included.
  - **Moderator:** from the request, `round_robin` by default like
    `ChannelTemplate`.
  - **Permission:** creating and deleting count as changing the session
    (`UpdateSession`), so only the owner may do it. Separate actions are
    introduced only when contributors are to be allowed.
  - **The last channel** may be deleted; a session without channels is
    harmless and can get a new one.
  - **State:** deleting a channel also deletes its channel state and the
    agents' channel states, through new `delete_…` methods of the
    `StateStore`. `delete_session` uses them too, since it leaves the same
    orphans today.
  - **API and UI:** `POST /sessions/{id}/channels` with
    `{ name, moderator?, participants }` and
    `DELETE /sessions/{id}/channels/{cid}`. In the web UI, "+" on a session
    opens a dialog for name, moderator and participants, and the channel's
    menu gets "Delete".
  - Agents creating channels stay out until
    [12](12-user-and-agent-permissions.md).
- **2026-09-25** — Implemented as decided above, with these details:
  - `SessionLoop::create_channel(context, ChannelSpec)` and
    `delete_channel`; `SessionController::add_channel` and `remove_channel`
    store the session under a write lock, which `update_name` now takes too,
    so that concurrent changes cannot overwrite each other. For the lock,
    `caelumora-model` depends on `tokio` (feature `sync`).
  - `SessionLoop::run` receives every channel to start over a queue, those
    of the session and those created later, and starts them in its
    `JoinSet`, so stopping the session still stops all of them. A deleted
    channel's task is aborted; one deleted before it started is never
    started. `run` no longer ends when the last channel ends, since a
    session without channels can get new ones.
  - Invalid requests are a `SessionError` (a new user-facing error type,
    added to `user_message`), refused before anything is stored: no name,
    unknown moderator, someone who is not a participant of the session, and
    also a name the session already has, because `channel_by_name` assumes
    names are unique. REST answers `400 invalid_channel`, `409 channel_exists`
    and `404 channel_not_found`. `RuntimeManager::new_moderator` reports an
    unknown kind as `SessionError::UnknownModerator`.
  - `SessionLoop::channels()` answers in the order of the session document.
    It used to come from a `HashMap` and could put a new channel before
    `main`.
  - The session response (`SessionSnapshot`) carries the session's
    `participants`, for the dialog.
  - **Bug found on the way:** `EntityStore::delete_session` deleted the
    channel state in the *entity* store, but entity and state store are
    configured separately (`examples/config.yaml` uses two directories), so
    the real state was left behind. Deleting state is now the `StateStore`'s
    (`delete_channel_state`, `delete_session_state`), called by
    `SessionManager::delete_session` and `SessionLoop::delete_channel`.
  - The event stream of a channel held it strongly in its resync closure and
    would have stayed open after deletion until its token expired; it holds it
    weakly now and ends with the channel.
  - Web UI: "New channel" under a session's channels opens a dialog for
    name, moderator and participants (preselected: all of the session's). A
    request error such as a taken name is shown in the dialog. Every channel
    has a menu with "Delete". If the open channel disappears, deleted here or
    elsewhere, the selection is cleared.
  - Verified in the browser: a duplicate name is refused in the dialog; a
    channel with only Sassy was created, opened and answered; deleting the
    open channel cleared the view, removed it from the stored session and
    deleted its state in the separate state store. Error codes and ordering
    were checked through the API from the page.
