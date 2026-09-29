# 106 — Small findings in the REST server

**Status:** done
**Area:** User interaction

## The problem

- `SessionResponse` is built by hand four times (`sessions.rs:48-52, 71-75,
  128-132`, `events.rs:107-113`) and carries `name` twice (`name:
  meta.name.clone(), metadata: meta`); the web UI reads only `id, name`. The
  "prefer the running session" branch in `list_sessions` (`sessions.rs:64-69`)
  is dead: renaming stores first, store and live object are identical.
- `AppState.state_store` is unused (compiler warning); `entity_store` is the
  same handle as `runtime_manager.entity_store()`.
- `GET /templates` returns `SessionTemplate` raw (`routes/templates.rs:8`),
  including `security_attributes` and `commands`; the UI reads
  `metadata.name`. Issue [42](42-export-import-chats.md) wants DTOs, never
  model types, on the wire. `templates.rs:2` uses `axum::Json` instead of
  `crate::error::Json` - the one deviation from the AGENTS.md rule.
- `kind` is a `String` with a fixed vocabulary (`"user"/"agent"/"approver"`)
  in `ParticipantResponse` and `ChannelMemberResponse`; an `enum` with
  `rename_all` would fix the contract with the UI.
- UUID → `ActorId` is done two different ways (`channels.rs:293` vs.
  `438-440`).
- `authenticate` recomputes `expires_at` (`auth.rs:267`) that
  `session_cookies` just computed; `kid` is the *name* of the secret (`:91`)
  and reads like the secret. `me` for an unknown user answers 401 without
  clearing the cookie; `authenticate` clears it.
- `auth.rs:139` `expect(...)` on the cookie `HeaderValue`.
- Stale docs: `GlobalSnapshot` "Unfiltered for now … issue 11" (it is
  filtered); `user_actor_of` "Until participants can be added (46)";
  `KnowledgeRefResponse.source` lists three values, the code emits
  `"memory"` too.
- Bind address hard-coded (`main.rs:101`, `127.0.0.1:3000`); no `server`
  section in `AppConfig`.
- `Cargo.toml`: `dashmap` and `tracing-subscriber` are not used in `src`.

## Why it matters

Individually small; together the list a reader of the REST crate trips
over. Recording them here keeps them from being found again.

## Considerations

`impl From<&Session> for SessionResponse` and drop `metadata`; `AppState {
auth, session_manager, runtime_manager }`; `TemplateResponse { id, name,
description }`; a `ParticipantKind` enum; `session_cookies` returns
`(cookies, expires_at)`; the rest as listed.

## Done when

The list is worked through; deliberate keeps are noted here.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 5.1.4, 5.2.4, 5.2.5, 5.5).
- **2026-09-29** — Worked through:
  - `SessionResponse` is `{id, name}`, made by `From<&Session>`, and the
    dead "prefer the running session" branch of `list_sessions` is gone;
    `SessionSnapshot` lost its `metadata` too, which the web UI never read.
  - `AppState` is `{auth, runtime_manager, session_manager}`; the handlers
    get the store from the `RuntimeManager`.
  - `GET /templates` answers `TemplateResponse {id, name, description}`
    through `crate::error::Json`; the web UI reads the flat fields.
  - `kind` is `ParticipantKind` (`user`/`agent`/`approver`, the same JSON),
    for channel members derived from the `ActorId` instead of the actor's
    kind string; the web UI types it as a union.
  - One lookup of an actor by the UUID in a path (`actor_in`).
  - `session_cookies` returns the expiry with the cookies; `kid` is
    `key_id`, documented as the secret's name; `/auth/me` for a user no
    provider knows clears the cookie, like a renewal; the cookie
    `HeaderValue` is built without `expect` - a failure is logged, and
    clearing cookies goes through one `clear_session`.
  - The stale docs are fixed (`GlobalSnapshot`, `user_actor_of`,
    `KnowledgeRefResponse.source` with `"memory"`).
  - The bind address is `AppConfig::server.address`, default
    `127.0.0.1:3000` (commented in `examples/config.yaml`). Kept on
    purpose: the web UI's Vite proxy points at port 3000, the default.
  - `dashmap` and `tracing-subscriber` are gone from the crate.
  - Beyond the list: `ApiError::status`, which only tests use, is
    `#[cfg(test)]` - the crate builds without warnings.
  Checked against a running server: login, templates, creating and
  listing sessions, a session's participants and members.
