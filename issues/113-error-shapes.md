# 113 — "Forbidden" and "not found" have three shapes each, and `ChannelError` lives in the wrong crate

**Status:** open
**Area:** Maintenance

## The problem

`StoreError::PermissionDenied` is a unit variant,
`SecretError::PermissionDenied(String)` carries a text,
`ChannelError::Forbidden { action, channel }` carries fields.
`StoreError::NotFound { entity, id }` vs. `SecretError::NotFound(String)`.
`TemplateError::Store(#[from] StoreError)` wraps a third. `ChannelError` is
defined in `caelumora-model` but constructed only by the runtime
(`channel.rs`, `session.rs`) and mapped only by the REST server.

## Why it matters

`caelumora_runtime::error::is_forbidden` and `user_message` have to know
every shape; the REST error mappers ([103](103-rest-error-mappers.md))
repeat that knowledge. One shape per meaning makes both a table.

## Considerations

Align `SecretError` with `StoreError` (`NotFound { entity: "secret", id }`,
`PermissionDenied` without text - the reason is never shown to users
anyway). Move `ChannelError` to the runtime next to `SessionError`. Small.

## Done when

Each of "forbidden" and "not found" has one shape across the domain
errors, and no error type lives in a crate that never constructs it.

## History

- **2026-09-28** — Split off from [97](97-small-model-unifications.md)
  (structure review, 3.2.8).
