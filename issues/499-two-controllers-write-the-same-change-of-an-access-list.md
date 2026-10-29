# 499 — Two controllers write the same change of an access list

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `ProjectController` and `WorkspaceController` each have a `change` that prunes the groups gone from the access list and a `change_access` that changes the list and announces it, the same but for the type they write.

## The problem

Since issue [492](492-a-document-is-written-under-two-locks.md), both controllers change their
document through `Writing::change`, each with two helpers of its own: `change`, which tells the
groups gone from the access list apart before the change (`group::gone_groups`) and drops them in
it (`group::prune_groups`), and `change_access`, which changes the access list and announces
`AccessChanged`. Before 492 they had the same pair, `store` and `store_access`. The two differ only
in the document's type and the event's.

A generic function cannot be written today: it needs to change the access list of any document,
and the model offers it only to read (`Secured::security_attributes(&self)`). `Artifact` has its
own way to change its lists (`access_lists_mut`), which the artifact manager prunes by.

## Why it matters

A third kind with an access list and a controller would copy them a third time; a change of how
gone groups are pruned is made twice already.

## Considerations

- **A way to change the access lists on the model's trait.** `Secured` - or a trait beside it -
  gains a method that lends the lists mutably, as `access_lists_mut` does for an artifact. Then one
  generic function beside `gone_groups` and `prune_groups` writes the change for any kind, and the
  artifact manager uses it too.
- **Leave it.** Two copies of a short pair, each beside the kind it is about.

## Done when

- Changing an access list and pruning the gone groups from it is written once for every kind that
  has a controller.

## History

- **2026-10-29** — Opened from the review of the second step of issue
  [492](492-a-document-is-written-under-two-locks.md), which rewrote both pairs and kept the copy.
