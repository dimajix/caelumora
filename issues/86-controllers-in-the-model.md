# 86 — The five controllers are orchestration in the model, built five ways

**Status:** done
**Area:** Code structure
**Epic:** [74](74-refactoring-epic.md)
**Summary:** The five controllers write to the store and publish events from inside the model, each built differently.

## The problem

`SessionController` (`caelumora-model/src/session.rs:20-155`), `ChannelController`
(`channel.rs:50-182`), `AgentController` (`agent.rs:24-99`), `UserController` (`user.rs:24-79`) and
`ApproverController` (`approval.rs:17-40`) hold live state (`ArcSwap`), write to the `EntityStore`,
publish `GlobalEvent`s and serialise with `tokio::sync::Mutex`. That is what ARCHITECTURE.md assigns
to the runtime; the model depends on `tokio/sync` only for them, and the runtime already has
`controller.rs` as their factory.

They also differ without reason: `SessionController::new` is `async` and returns `Result` with
nothing to await or fail (`session.rs:31-44`); `ChannelController.entity_store` is an `Option`,
`None` only in tests; `AgentController` has no `write_lock`, session and channel do;
`UserController` has no store, and `update_username` has no caller; `process_global_event` is
`async` on session and agent, sync on user; `SessionController::process_global_event` is six `//
TODO` branches out of seven (`session.rs:129-155`), and `SessionLoop::run` subscribes to the session
topic only for it. `ApproverController` wraps an `Approver` in an `ArcSwap` with no store and no
event - a wrapper without a purpose.

## Why it matters

The model is meant to hold data and pure rules. A reader looking for "who writes a channel" has to
look in the model, and each controller teaches a different construction.

## Considerations

- Move the five to `caelumora-runtime/src/controller.rs` (or one file each). The model keeps
  `Session`, `Channel`, `Agent`, `User`, the `*UpdateEvent` enums and `participant_attributes`.
  About 14 `use` lines change; the two store tests move with them.
- One shape: `new(entity, publisher, store)` sync, `write_lock` everywhere, `process_global_event`
  sync, one `store_and_announce` helper. Drop the TODO branches (a channel is registered through
  `add_channel`/ `remove_channel`, not through events) and `ApproverController`.
- The consistent next step would be `config.rs`, `secret*.rs`, `user_provider*.rs` and `store_impl/`
  as well - then the model holds only data, rules and the store traits. That is a separate decision.

## Done when

- `caelumora-model` has no `tokio` dependency and no type that writes to a store.
- The remaining controllers share one constructor and one update shape.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 3.1.4).
- **2026-09-29** — Done. The controllers live in `caelumora_runtime::controller`, one file each,
  with the `ControllerManager` in `mod.rs`; the model keeps the documents, the `*UpdateEvent` enums
  and `participant_attributes`, and no longer depends on `arc-swap` or tokio's `sync` (the runtime
  now declares `sync` itself instead of getting it through feature unification). One shape: built
  from the document, the event publisher and the store, all synchronously; a write lock everywhere
  (`AgentController` got one); `process_global_event` synchronous everywhere. `ChannelController`'s
  store is no longer optional - the approver tests store their channel first. Removed: the six `//
  TODO` branches of `SessionController::process_global_event` (a channel is registered through
  `add_channel`/`remove_channel`), `UserController::update_username` and with it the user
  controller's event publisher (it announced nothing else), and `ApproverController`. The
  `SessionLoop` still subscribes to its session topic, for renames announced elsewhere. The two
  store tests that exercised controllers moved along and now check a rename. The next step named
  above - moving `config.rs`, the secret and user providers and `store_impl/` out of the model too -
  is a separate decision and was not taken.
