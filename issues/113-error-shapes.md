# 113 — "Forbidden" and "not found" have three shapes each, and `ChannelError` lives in the wrong crate

**Status:** done
**Area:** Maintenance
**Epic:** [74](74-refactoring-epic.md)
**Summary:** "Forbidden" and "not found" have three shapes across the domain errors, and `ChannelError` lives in a crate that never constructs it.

## The problem

`StoreError::PermissionDenied` is a unit variant, `SecretError::PermissionDenied(String)` carries a
text, `ChannelError::Forbidden { action, channel }` carries fields. `StoreError::NotFound { entity,
id }` vs. `SecretError::NotFound(String)`. `TemplateError::Store(#[from] StoreError)` wraps a third.
`ChannelError` is defined in `caelumora-model` but constructed only by the runtime (`channel.rs`,
`session.rs`) and mapped only by the REST server.

## Why it matters

`caelumora_runtime::error::is_forbidden` and `user_message` have to know every shape; the REST error
mappers ([103](103-rest-error-mappers.md)) repeat that knowledge. One shape per meaning makes both a
table.

## Considerations

Align `SecretError` with `StoreError` (`NotFound { entity: "secret", id }`, `PermissionDenied`
without text - the reason is never shown to users anyway). Move `ChannelError` to the runtime next
to `SessionError`. Small.

## Done when

Each of "forbidden" and "not found" has one shape across the domain errors, and no error type lives
in a crate that never constructs it.

## History

- **2026-09-28** — Split off from [97](97-small-model-unifications.md) (structure review, 3.2.8).
- **2026-09-29** — Done for the part that had a reason: `ChannelError` moved from
  `caelumora_model::channel` to `caelumora_runtime::channel`, next to the `ChannelLoop` that raises
  every one of its variants and the `SessionError` it sits beside in `error::user_message`; REST and
  the tests import it from there. Not changed, deliberately:
  - `SecretError`. Its shape differs from `StoreError` for a reason - a secret is found by its name,
    not as an entity with an id - and the argument above does not hold for it: `user_message` and
    `is_forbidden` never see a `SecretError` (the MCP component wraps it into a tool error, the REST
    server matches `NotFound` once at start). Aligning it would be churn without a reader who
    profits.
  - `TemplateError::Store` wraps a `StoreError` because instantiating a template does store I/O in
    the model; it goes with [88](88-template-instantiation-in-the-model.md), not here. "Forbidden"
    is recognised in one place, `error::is_forbidden`, which is the one that matters to callers.
