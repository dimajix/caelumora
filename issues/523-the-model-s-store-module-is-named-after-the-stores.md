# 523 — The domain's module `store` reads as the stores, and holds the error of the stores

**Status:** open
**Type:** question
**Priority:** low
**Area:** Code structure
**Summary:** The module `store` of `caelumora-model` holds what the domain says of being stored and asked for - `Stored`, `Revision`, `EntityKind`, fields, filters and pages - but its name reads as the stores of `caelumora-store`, and it holds `StoreError`, the error of the stores.

## The problem

Since issue [517](517-the-model-offers-libraries-what-only-the-kernel-may-decide-by.md) the stores
are `caelumora-store`'s; `caelumora_model::store` keeps "the words of the stores", as its module
docs say (`caelumora-model/src/store/mod.rs:1`). Beside `caelumora_store` the name reads as the
stores themselves. What the module holds:

- **What an entity is to be stored:** `Stored`, `Updatable`, `EntityKind`, `Revision`,
  `Enclosing`, `NoEntity`, `Address`, and `Journaled` for a state that tells of its changes.
- **What a query asks** (`store::query`): `Field`, `FieldPath`, `NoField`, `FieldValue`, `Filter`,
  `Query`, `Cursor`, `Page`, `ordered_after`, `meeting`.
- **What a page of a history is filtered by** (`store::history`): `HistoryField`, its filters,
  `KeptItem`.
- **How a store fails:** `StoreError`, `StoreResult`, `StoreResultExt`.

The maintainer proposed to name it `fields` and to move `StoreError` to `caelumora-store`.

## Considerations

- **`fields`** names the query part well, but not `Stored`, `Revision` or `EntityKind`, which are
  the larger half and are implemented by the domain's types. Two ways to give each part a fitting
  name:
  - **Rename the module as a whole**, to a name that says "what is stored, and how it is asked
    for" without reading as the stores: `stored` (after its central trait). `persistence` is taken
    by `caelumora_store::Persistence`.
  - **Split it**: the queries (`Field`, `Filter`, `Query`, `Page`, `Cursor`, and the history's
    fields) into a module of their own, `query`; what an entity is to be stored stays under the
    renamed module. Two modules, each describable in one sentence.
- **`StoreError` cannot simply move.** The API that libraries are given names it:
  `caelumora_api::runtime` returns `StoreResult` (`load_channel_state`, `store_channel_state`,
  `transcript_message`), `ComponentError::Bundle` and `WorkspaceError::Store` carry it,
  `caelumora_api::error::store_message` judges it; `caelumora-agents`, `caelumora-artifacts` and
  `caelumora-skills` match on it. A library links only `caelumora-api` and the domain (issue
  [516](516-the-runtime-trusts-the-agents-linked-into-it.md)); with `StoreError` in
  `caelumora-store`, the API would link the stores, and every library with it. It can move only if
  the API stops naming it - an error of the API's own, into which the kernel translates what a
  store says -, which is a change of the API of its own, with a reason beyond the name.
- **With issue [522](522-two-crates-named-model.md)** the crate becomes `caelumora-domain`, and
  `caelumora_domain::store` reads less like the stores than `caelumora_model::store` does; the
  question remains, but is less pressing. Best decided after 522, and done in one step with it or
  after it, not before.

Recommended: split off `query`, rename the rest `stored`, and leave `StoreError` with it.

## Done when

- The module's name, or the names of its parts, say what they hold, decided with the maintainer.
- Where `StoreError` lives is decided, and the API keeps to what a library may link.

## History

- **2026-11-02** — Opened at the maintainer's request, after the discussion of the names of the
  crates (issue [522](522-two-crates-named-model.md)).
