# 455 — Preconfigured components are configuration kept as entities, the one kind without a UUID

**Status:** in progress
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** The preconfigured components are copied once from the seed into a store of their own, read only to resolve a `ref` and written by nothing else, and theirs is the one kind whose id is not a UUID - so no store can key every kind by one.

## The problem

The preconfigured components (`StoredComponents`, issue [270](270-personality-inheritance.md)) are
kept in the `ComponentStore`, a store of the documents of the kind `components` beside the
entities, checked as they are (`CheckedStore`). What uses it:

- **Writing:** the seed alone (`Seed::apply`), which creates a component whose id is not there yet
  and never changes one that is - as it seeds the personalities, the templates and the sections of
  the plugins: demo data put into the stores once (issue 105).
- **Reading:** the `ComponentRegistry` alone, resolving a `ref` in the name of whoever builds
  (`get_components`). No route of the REST server and no controller creates, changes, lists or
  deletes a component; `update_components`, `delete_components` and `list_components` have no
  caller but tests.

So they are configuration, kept as entities. And theirs is the one kind whose id is a `String`, a
name chosen in the seed (`"basics"`, `"search"`): every other kind - of the core and of the
plugins, `SkillId` and `ArtifactId` among them - is identified by a UUID (`define_id!`).

That stands in the way of keying every kind alike. The Postgres store of issue
[332](332-policy-as-a-predicate.md) needs, per kind, a table of the access entries of its
entities, joined with the groups; measured on 100 000 entities, it decides a page of a reader in
any number of groups in under 70 ms, where the table of the kind is walked for seconds without it.
With every id a UUID, the column of the id - in the table of a kind, of its access entries, of
the groups joined - is a `uuid` of 16 bytes instead of a text of 37, compared without a cast; and
the model could say so, an entity's id being a UUID (`Stored::Id`, `Document::id`).

## Considerations

- **A. A UUID and a unique name.** A component gets a UUID as its id; what is its id now becomes
  its name, unique in the global scope (issue 250), and a `ref` is resolved by name
  (`find_by_name`). The smallest change; the components stay entities, and the seed still does not
  reach what it seeded once.
- **B. Configuration.** The components are held where the configuration is - read from the seed
  files, or from `AppConfig` - by the `ComponentRegistry`, which resolves a `ref` from them and asks
  the `AccessPolicy` about their security attributes as the store asks now. The `ComponentStore`,
  its implementation on `CheckedStore`, the kind `components` in every backend and its part of the
  suite go; what a seed file says is what holds, at every start. What is lost is creating and
  changing a component while the program runs, which nothing does.
- **What 270 relied on.** Issue 270 kept the bundles in the store for an owner and contributors,
  actions of their own (`GetComponent` and the others) and a read afresh at every build in the
  agent's name. The first two need nobody but the administrator while only the seed writes; the
  last holds as well for configuration that is read at start. Whether a user is to share bundles
  of their own later is the question that decides between A and B.
- **The ids of all kinds as UUIDs.** Once no kind has another id, `Stored::Id` can require one
  (`Into<Uuid>` and back, or a bound on `define_id!`'s types), `Document::id` become a `Uuid`, and a
  storage key by it: the JSON files keep their names, memory keys by it, Postgres makes the column
  a `uuid` - a new form of the tables of the kinds, so stored Postgres schemas are deleted. Ordered
  as Postgres orders a `uuid`, bytewise, the ids keep the order of their text, so cursors
  (`DocumentStorage::scan`) do not change.

## Decision

Neither A nor B as weighed, but A carried through: the preconfigured components become an entity
kind like every other - a personality, a template, a skill -, since they are what a personality is
made of, and a user who keeps personalities of their own one day keeps the bundles they include as
well. As an ordinary kind, the special path goes:

1.  **`ComponentBundle`**, the kind `component_bundle`, in place of `StoredComponents`: an id of its
    own (`ComponentBundleId`, `define_id!`), a `ScopedName` unique in its scope (issue 250), its
    `components`, its `security_attributes` and the mark of a deletion (issue 164) - `Stored`,
    `Secured`, `Scoped` and `Deletable`, as `Personality` is. Named after what it is - a bundle of
    components that several personalities include, as 270 called it - rather than after where it
    was kept.
2.  **The `ComponentStore` goes**, with `Persistence::component_store`, its implementation on
    `CheckedStore` and its part of the suite: the bundles are an `EntityStore` of `EntityStores`,
    asked as every kind is. Deleting marks it, as every kind's does; the rule that a preconfigured
    component is removed for good (issue 164) is reversed, since it stands on its own now.
3.  **A `ref` names a bundle by its name**, `params: { name: …, scope: … }` - a `ScopedName`, the
    scope global unless named -, found in exactly that scope (`EntityStore::find_by_name`) in the
    name of whoever builds, as it is read now. Looking through the scopes seen from a place, as for
    skills (`Scope::seen_from`), is left until users keep bundles of their own.
4.  **The seed** writes a bundle with its `id` and `name`, as it writes a personality.

Then the ids of every kind as UUIDs, as considered above.

## Steps

1.  **Component bundles as a kind**, as decided.
2.  **Every id a UUID**: `Stored::Id` a UUID under a type of its kind's, `Document::id` a `Uuid`,
    the storages keyed by it, the column of the id a `uuid` in Postgres - a new form of the tables
    of the kinds.

## Done when

- The preconfigured components are a kind of entity, `ComponentBundle`, kept, checked and found by
  name as every kind is, and the `ComponentStore` is gone.
- Every kind is identified by a UUID, and the stores key it as one.
- `docs/STORAGE.md`, `docs/COMPONENTS.md`, `docs/CONFIGURATION.md` and `docs/ARCHITECTURE.md`
  describe it.

## History

- **2026-10-23** — Opened while designing the Postgres side of
  [332](332-policy-as-a-predicate.md): measured, an access table per kind joined with the groups
  is the form that scales, and keying it by `uuid` needs every kind to have a UUID; the
  preconfigured components are the one that has not, and they behave as configuration.
- **2026-10-23** — Decided with the maintainer: the preconfigured components become a kind of entity
  like every other, `ComponentBundle`, found by name - so that users may keep bundles of their own,
  as they will personalities -; the `ComponentStore` goes. That a seed file reaches nothing it
  seeded once holds for every kind it seeds, by design, and is no reason of its own.
- **2026-10-23** — Step 1: `ComponentBundle` a kind like every other - `Stored`, `Scoped`,
  `Deletable`, a member of the scope it lies in, run through the store suite -; a `ref` names it by
  `name` and `scope`, found by `find_by_name`; the `ComponentStore` gone, and with it the last kind
  removed for good, so `CheckedStore::remove` and `DocumentStorage::remove` went too; the seed's
  section is `component_bundles`, and "preconfigured components" are called component bundles
  everywhere.
