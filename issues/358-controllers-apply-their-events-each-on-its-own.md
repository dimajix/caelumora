# 358 — The controllers apply, store and announce their events each on their own, and the loops forward them unevenly

**Status:** done
**Area:** Code structure

## The problem

[86](86-controllers-in-the-model.md) gave the five controllers one construction. What they do with
an update event is still written per controller:

- `SessionController` has `apply(event)`, `update(ctx, event)` - lock, apply, store, take over,
  announce - and `process_global_event` (`controller/session.rs:130`, `:176`, `:211`);
  `AgentController` has the same three, differing in the type (`controller/agent.rs:88`, `:104`,
  `:115`); `ChannelController::update` has the `match` inline and announces with two
  classifications (`controller/channel.rs:234`); `UserController` applies two events in
  `process_user_update` without an `apply`. `SessionController::add_channel` and `remove_channel`
  go through `store` instead of `update`, which is where `updated_at` is set (`:200`) - `update`
  does not set it, so a rename leaves `updated_at` as it was while adding a channel moves it.
- The loops wrap the controllers: `ChannelLoop::update_name`, `update_language`,
  `update_description`, `update_uses_workspace`, `update_approval_rules` (`channel/settings.rs`)
  and `SessionLoop::update_name`, `update_privacy`, `update_approval_rules`, `update_description`
  (`session/settings.rs`) each forward one call. Four of them log `info!`, `update_name` and
  `update_privacy` do not; the controllers log nothing.

## Why it matters

Every controller teaches its own `update`, and the one difference that matters - the channel
announcing to who reached it as it was - is hidden among differences that do not. A reader cannot
tell from a loop's `update_*` whether it adds anything to the controller's. `updated_at` is set or
not depending on the path.

## Considerations

- With `apply` in the model ([353](353-rules-of-the-model-computed-in-the-runtime.md)), the
  controllers become alike enough for one shape: a generic `Controlled<T, E>` holding the document,
  the lock, the store and the publisher, with `update(ctx, event)` doing lock, `T::apply`, store,
  take over, announce; what a kind announces with (the channel's two classifications, a session's
  attributes) is the one method a kind writes. `process_global_event` is the generic's too. Whether
  this is one generic type or one shared `update` per controller is to be decided by what the
  generic costs in `where` clauses.
- `updated_at` is set in `update` for every kind that has it, or by `apply` in the model; `store`
  goes.
- Logging once, in the controller's `update` with the kind, the id and `ctx.caller_id`; the loops'
  wrappers become one line each - or go, if REST may hold the `ChannelController` through the loop.
  Against the latter: `reclassify` and `set_workspace` do more than forward and must stay on the
  loop, so a mixed API would follow. Keep the wrappers, drop their logging.

## Done when

- One `update` shape for the controllers, with what differs per kind in one method each.
- `updated_at` moves with every change, or it is decided that it does not.
- The loops' `update_*` add nothing to the controllers' but the forwarding; logging is the
  controllers'.
- No behaviour changes but `updated_at`; the controller and settings tests pass.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
- **2026-10-15** — Done, as one generic type by the maintainer's decision, with `updated_at` moved
  by every change and the logging once, in the controllers.
  - In the model, a trait `Updatable: Stored` (`store/mod.rs`): the kind's update event
    (`Update`), `apply` - the `apply` of `Session`, `Channel`, `Agent` and `User` moved into it -
    and `touch`, which `Session` and `Workspace` answer with their `updated_at`. `Workspace` has an
    `apply` of its own now; whether a classification may be given stays the runtime's to ask
    before.
  - In the runtime, `controller/document.rs`: `LiveDocument<T>` holds the address, the live copy,
    the store, the publisher and the lock - the fields every controller had -, with `update`
    (lock, apply, touch, store, take over, announce), `take_over` (today's
    `process_global_event`), `reached` and `reload` (read as stored). `Writing<'_, T>` is a change
    under way, under the lock: `current` or `reached`, `apply`, `store`, `announce`,
    `announce_update`, `announce_also_to`; what a controller does besides it does between them -
    the participants' group of a session and a channel, the membership events, the workspace's
    checks.
  - What a kind announces with needed no method of its own: every change is announced to who
    reached the document as it was classified when the change began and as it is
    (`Stored::classification`), once if they are the same - the channel's and the workspace's two
    classifications, nothing for a kind that is not classified.
  - `SessionController::store` and `ChannelController::store` are gone, so are the five
    controllers' `write_lock`, `event_publisher` and store fields and their `id` fields (the
    address answers it). `AgentController::update_name` goes through `update`.
  - Every stored change is logged once, `info!` with the kind, the id and who changed it, not what
    changed - a name or a briefing is someone's text. The loops' `update_*` are one line of
    forwarding each, without logging; `reclassify` and `set_workspace` keep theirs.
  - The `UserController` holds no `LiveDocument`, on purpose: it does not write the user, and the
    profile is created with its first change and announced as it stands. It takes the user over
    with the same `take_over` and logs a change of the profile with the same `log_changed`.
  - `updated_at` of a session now moves with every change, not only with its channels; no stored
    data has to be deleted. STORAGE.md describes the shape.
