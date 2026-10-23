# 456 — `Stored` and `Deletable` are two traits, though every stored kind is deletable

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Persistence
**Summary:** Whether a kind is marked when it is deleted is a trait of its own beside `Stored`, with a default that says "never deleted", though once the component bundles are a kind like every other no stored kind is removed for good any more.

## The problem

A kind the stores keep is `Stored`; one that deleting marks rather than removes is `Deletable`
besides (issue [164](164-soft-deletion-and-former-members.md)), and `Stored::deleted` answers
`None` by default, "never for a kind that is not `Deletable`". The split was made for the
preconfigured components, which were removed for good. With issue
[455](455-components-configuration-kept-as-entities.md) they become `ComponentBundle`, a kind
like every other, marked when it is deleted - and with them goes the last stored kind that is not
`Deletable`, and the path that removed one for good (`CheckedStore::remove`,
`DocumentStorage::remove`).

What is left is a distinction without a case:

- Every one of the stored kinds - of the core, of the plugins (skills, artifacts) and of the tests
  (`suite::Folder`, `Impostor`, `Groupie`) - implements both traits, and writes `deleted` and
  `set_deleted` in two places.
- `Stored::deleted` has a default that no kind may rely on, since every kind is `Deletable`: a kind
  that forgot to implement it would read as never deleted, and its deleted documents as present.
- The stores and the factory are bounded by `T: Deletable` (`EntityStore`,
  `EntityStoreFactory::entity_store`, and some 9 bounds in all), which says nothing `Stored` would
  not say.

## Considerations

- **Fold `Deletable` into `Stored`.** `deleted` and `set_deleted` become required methods of
  `Stored`, without a default; the bounds `T: Deletable` become `T: Stored`; `Deletable` goes,
  and with it the second `impl` of every kind. What `docs/STORAGE.md` says already ("every kind")
  is then what the code says.
- **Keep the split for a kind to come** that is removed for good: none is in sight - what is
  stored is content, an identity or a managed definition, all of which issue 164 marks -, and a
  trait without a case is a concept a reader must know for nothing (AGENTS.md, "Nothing without a
  user").
- **The name.** `deleted()` and `set_deleted()` stay as they are named; `Removal` stays the type of
  the mark.

## Done when

- No stored kind implements a trait of its own for being deleted, and no default answers that one
  never is.
- `docs/STORAGE.md` describes it.

## History

- **2026-10-23** — Opened from a finding of step 1 of
  [455](455-components-configuration-kept-as-entities.md), with which the last stored kind that
  was removed for good becomes one that is marked.
- **2026-10-23** — Done as the first consideration says: `deleted` and `set_deleted` are required
  methods of `Stored`, without a default, `Deletable` is gone, and with it the second `impl` of
  every kind; the stores, the factory, `Unowned`, `LiveDocument` and `Writing` are bounded by
  `Stored`. The two test kinds that took the name of the groups (`Impostor`, `Groupie`) read and
  set the mark of the note they wrap, where they had ignored it. `docs/STORAGE.md` says that every
  kind that is `Stored` implements both.
