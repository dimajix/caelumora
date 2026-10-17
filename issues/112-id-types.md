# 112 — Two id types are hand-written, and the approver has none

**Status:** done
**Area:** Maintenance
**Epic:** [74](74-refactoring-epic.md)
**Summary:** `ToolId` and `SessionTemplateId` are written by hand, and `ActorId::Approver` holds a bare `Uuid`.

## The problem

`define_id!` (`caelumora-model/src/macros.rs`) generates seven id types; `ToolId` (`tool.rs:10-12`)
and `SessionTemplateId` (`template.rs:119-121`) repeat the same derive block by hand.
`ActorId::Approver(Uuid)` (`actor.rs:21`) holds a bare `Uuid` instead of an `ApproverId`, which
makes the runtime write `approver.id.0` in four places. The macro implements `Into<Uuid>` instead of
`From<$name> for Uuid`, which clippy flags.

## Why it matters

An id type is the one place a mix-up between kinds is caught by the compiler; a bare `Uuid` in
`ActorId` is where it is not.

## Considerations

Use the macro for all nine, add `ApproverId`, implement `From`. About six call sites. `ToolId` has
no caller at all ([107](107-public-interface.md)) and may simply go.

## Done when

Every id in the model comes from `define_id!`, and `ActorId` holds only id types.

## History

- **2026-09-28** — Split off from [97](97-small-model-unifications.md) (structure review, 3.2.10).
- **2026-09-29** — Done. `SessionTemplateId` comes from `define_id!`; `ToolId`, which nothing used,
  is gone. `ActorId::Approver` holds an `ApproverId`, so `approver.id.0` is `approver.id` at the
  five places that built one. The macro implements `From<$name> for Uuid` instead of `Into`. The
  JSON form is unchanged - the id types are `#[serde(transparent)]` - so stored data and the web UI
  read as before.
