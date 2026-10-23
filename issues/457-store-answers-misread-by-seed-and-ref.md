# 457 — A deleted seeded document fails every start, and a failing store reads as a configuration error

**Status:** open
**Type:** bug
**Priority:** low
**Area:** Persistence
**Summary:** The seed asks whether a document is there with `get`, which does not find one marked deleted, and then fails to create it again, so deleting a seeded personality makes every later start fail; and a store failure while a `ref` is resolved is reported as an invalid configuration that names no bundle.

## The problem

Two places read an answer of the stores as something it is not.

**The seed.** `Seed::apply` (`caelumora-runtime/src/seed.rs`) puts each document into the stores
that they do not have yet: it asks `get(id)`, and creates the document where that answers "not
found". But `get` treats what is marked deleted as absent (issue
[164](164-soft-deletion-and-former-members.md)), also for the system, while the id stays taken: the
`create` that follows fails with `StoreError::Conflict`. So once a seeded personality, session
template or - with issue [455](455-components-configuration-kept-as-entities.md) - component
bundle is deleted, every later start of the program fails while seeding. The skills' plugin seeds
its section the same way (`caelumora-skills/src/plugin.rs`). Before 455, a preconfigured component
was removed for good and simply seeded again, which is why the case was never met there.

**A `ref`.** `ComponentRegistry::resolve` (`caelumora-runtime/src/component/registry.rs`) finds the
bundle a `ref` names; "not found" and "permission denied" become `ComponentError::Unavailable`, as
they should, but every other `StoreError` - a backend that cannot be reached, a document that
cannot be read - becomes `ComponentError::InvalidConfig("no component bundle '…': …")`: a failing
store reported as an error of the configuration, with a message that says there is no such bundle.

## Considerations

- **What a deleted seeded document means.** It was deleted on purpose - by its owner, the system,
  or an administrator -, so seeding it again would undo that; the seed should leave it as it is,
  as it leaves one that was changed. Asked with `get_deleted` before `create` - or by one question
  of the stores, "is there a document of this id at all" -, for every seeded kind alike, the
  plugins' sections included (`SeedSection` says "each document it does not have yet, by id").
  Whether a purge should then let it be seeded again is a question of its own; purged, its id is
  free, and it would be.
- **One rule, not one per seeder.** The core and the skills' plugin each write the check; a helper
  of the seed that every `SeedSection` uses would keep them alike.
- **The error of a `ref`.** A variant of `ComponentError` that carries the `StoreError` of a store
  that failed - as `Unavailable` carries the one of a bundle that cannot be read -, so that the
  caller tells a broken store from a broken configuration, and the message names the store's
  failure rather than a missing bundle.

## Done when

- A seeded document that was deleted is not seeded again and does not fail the start, for every
  kind the seed and the plugins' sections write.
- A store that fails while a `ref` is resolved is reported as such.

## History

- **2026-10-23** — Opened from two findings of step 1 of
  [455](455-components-configuration-kept-as-entities.md): with the component bundles marked when
  deleted, the seed's check by `get` meets the case for one more kind, and the resolution of a
  `ref` was rewritten without changing how it reports a failing store.
