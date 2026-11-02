# 523 — The domain's module `store` reads as the stores, and holds the error of the stores

**Status:** in progress
**Type:** refactoring
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

## Decided

Discussed with the maintainer on 2026-11-02, after issue
[522](522-two-crates-named-model.md) renamed the crate to `caelumora-domain`; the recommendation
above was revised:

- **`fields` is not the name.** The fields of the kinds already lie with their kinds
  (`SessionField` with `impl Filter<SessionField>` in `session.rs`, `ChannelField`, `UserField`,
  `GroupField`, `ScopeField`); a module `fields` would hold the trait `Field` and `HistoryField`,
  and nothing of `Stored`, `Revision` or `EntityKind`.
- **The queries become a module of their own**, `query`: every kind has fields and every store
  answers queries; they lay under `store` only because the stores evaluate them.
- **The history's fields go to the history**: `HistoryField`, `KeptItem` and their filters beside
  `Recorded` and `Reader`, as `SessionField` stands beside `Session`.
- **The rest keeps the name `store`.** It is the pattern of `caelumora_domain::security` beside
  `caelumora_security`: the domain's module says what a thing has to carry, the crate of its name
  decides or keeps it. The crate's prefix tells the two apart, unlike two crate names a letter
  apart. `stored` would break the pattern, is the only participle among the domain's modules of
  nouns, and reads `stored::Stored` beside `chain::Stored<E>`.
- **`StoreError` stays in the domain**, beside `Stored`: the API given to libraries names it, and
  moving it would need an error of the API's own, a type without a responsibility of its own -
  as `Refused` lies in `caelumora_domain::security` for the same reason.

## Plan

One step without behaviour change; no stored data changes.

- **`query`.** `caelumora-domain/src/store/query.rs` becomes `caelumora-domain/src/query.rs`,
  declared `pub mod query;` in the crate's root; every `store::query` becomes `query` - imports,
  `$crate::store::query::FieldValue` in `macros.rs`, doc comments, STORAGE.md.
- **`history::filter`.** `caelumora-domain/src/store/history.rs` becomes
  `caelumora-domain/src/history/filter.rs`, declared `mod filter;` in `history` and re-exported
  (`pub use filter::{HistoryField, KeptItem};`), so that they are imported as
  `caelumora_domain::history::HistoryField`, as `caelumora_domain::session::SessionField` is.
- **`store`.** Its module docs name what is left: what an entity is to be stored, what a journaled
  state tells, and how a store fails; the queries and the history's fields are named where they
  went.
- **Tests.** None new; the build, the tests, clippy and the checks of the pre-commit hook.

## Done when

- The module's name, or the names of its parts, say what they hold, decided with the maintainer.
- Where `StoreError` lives is decided, and the API keeps to what a library may link.

## History

- **2026-11-02** — Opened at the maintainer's request, after the discussion of the names of the
  crates (issue [522](522-two-crates-named-model.md)).
- **2026-11-02** — Decided with the maintainer as above, and planned, on the branch
  `issue-523-store-module`.
