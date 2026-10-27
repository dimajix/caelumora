# 459 — What was deleted is reached by a flag and paired methods, and called a trash

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Persistence
**Summary:** Whether an operation of a store is about the entities there or those deleted is a `bool` passed down, and the access checks come in pairs that differ only in it, so that what makes reaching a deleted entity special - kept for the record, asked by a verb of its own - is nowhere one concept; and the documents call it a trash.

## The problem

A deleted entity is marked, not removed (issue [164](164-soft-deletion-and-former-members.md)):
it is kept so that it can be said later that it was there, who deleted it and when - a deletion is
the one operation that destroys evidence (issues [07](07-session-deletion-audit.md),
[140](140-audit-log.md)). That its owner may restore it is a side effect; the reason it is kept is
the record. So reaching what was deleted is always something particular: asked by a verb of its own
(`ReadDeleted`), the owner's alone by the ownership policy, and what an audit would watch.

In the code it is not one concept but a flag and pairs of methods:

- **A `bool` passed down.** `EntityStore::list` and `list_deleted` call `CheckedStore::page(…,
  deleted)`, which hands `deleted` to `DocumentStorage::scan`; `page` branches on it twice - for
  the admission (issue [332](332-policy-as-a-predicate.md)) and for each candidate.
- **Methods in pairs.** `Access` has `read` and `read_deleted`, `readable` and `readable_deleted`,
  `admission` and `admission_deleted`: each pair differs only in the mark it checks (the private
  `present`) and the verb it asks (`Get`, `ReadDeleted`).
- **The pairing is nowhere named.** That the deleted are read by `ReadDeleted` and the others by
  `Get` is repeated in every pair instead of being said once, so a new operation over what was
  deleted has to repeat it again.
- **Called a trash.** The documents, comments, routes and the web UI speak of "the trash" (about
  130 places), which says that what is there is waiting to be thrown away or put back - not that it
  is kept for the record.

## Considerations

- **A type for which entities an operation is about**, inside the store:

  ```rust
  /// Which of a kind's entities an operation is about: those there, or those deleted - kept for
  /// the record (issue 164).
  pub enum Presence { Present, Deleted }

  impl Presence {
      /// Whether `value` is among them, by its mark.
      fn holds<T: Stored>(self, value: &T) -> bool;
      /// What reading one of them asks: `Get`, or `ReadDeleted`.
      fn verb(self) -> Verb;
  }
  ```

  `page` and `DocumentStorage::scan` take it in place of the `bool`; each pair of `Access` becomes
  one method taking it (`admission::<T>(presence)`, `readable(value, presence)`, `read(found, id,
  presence)`); `present` goes into `Presence::holds`. The `EntityStore` keeps its methods by name
  (`get_deleted`, `list_deleted`), so that a caller still says at its call that it reaches what was
  deleted. What is particular about the deleted stays in the verb the policy decides, and is said
  once, where `Deleted` is matched - one place for an audit to hook into, too. The change is a
  restructuring without a change of behaviour; `scan` changes in every backend.
- **Its name.** `Presence { Present, Deleted }` says what the mark says. A name after the record -
  `Standing { Live, Retained }` - says why the deleted are kept; one after the trash says what it is
  not.
- **"The trash" renamed** everywhere it means what was deleted - the documents, the comments, the
  REST routes' descriptions and the web UI's words - to say that it is kept for the record and may
  be restored. A concept is renamed in the same change when what it means changes (AGENTS.md); here
  the meaning was always the record, and only the word is wrong. Whether this is one change with the
  type or one of its own is to be decided; the routes and the web UI make it the larger part.

## Done when

- Whether a store's operation is about the entities there or those deleted is a type of its own,
  and the access checks over what was deleted are not pairs of the others.
- What was deleted is called by a name that says it is kept for the record, in the code and the
  documents, or it is decided why "trash" stays.

## History

- **2026-10-23** — Opened during step 2 of [332](332-policy-as-a-predicate.md), which added a third
  pair to `Access` (`admission`, `admission_deleted`) and a second branch on `deleted` to
  `CheckedStore::page`. The maintainer pointed out that the deleted are kept for auditability and
  restoring them is a side effect, so reaching them is always something particular - and not a
  trash.
- **2026-10-27** — Decided: two steps, `Presence { Present, Deleted }` first, then "trash" renamed.
  Step 1: `Presence` (`store/mod.rs`) takes the place of the `bool` in `CheckedStore::page` and
  `DocumentStorage::scan`; `Access` has one `read`, `readable` and `admission` taking it, and
  reading, changing, deleting and purging share one check of mark, verb and clearance
  (`require_reached`). `ActionType::read_deleted` went with its last caller. `Document::deleted`
  stays a `bool`: it is the stored mark, which a storage compares with `Presence::marked`.
- **2026-10-27** — Step 2: "the trash" is gone from the code, the documents and the web UI's
  names; what was deleted is called so, or "the deleted". The REST API's module `trash` is
  `deleted`; its routes and their parameter (`?deleted=true`) were named so already. The web UI
  already showed "Deleted sessions", "Deleted channels" and so on, and keeps them; only its
  identifiers changed (`deletedVersion`). The icon `Trash2` of the delete actions stays: it is
  lucide's name for the picture, not a concept of ours.
