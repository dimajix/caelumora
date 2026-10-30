# 499 — Two controllers write the same change of an access list

**Status:** done
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
- **2026-10-29** — Done. The model lends the lists to change through `SecuredMut`, beside
  `Secured`: `security_attributes_mut`, and `access_lists_mut`, which an artifact widens by its
  versions' lists. The user is left out, whose attributes are fixed (`SYSTEM_OWNED`), so that
  `Secured` could not lend them mutably. `Writing::change` drops the entries naming a group that is
  gone itself, after the change and in every try, by `prune_gone_groups` again - the split into
  `gone_groups` and `prune_groups`, made only for a synchronous closure, is gone -, and
  `Writing::change_access` changes the access list and announces the kind's `AccessChanged`. Both
  controllers lost their pair. Every `LiveDocument` prunes now, the session's, the channel's and the
  agent's included, which issue 347 had left out; the maintainer decided so for every kind alike.
  An entry naming a session's, channel's or project's own group goes, too, if that group is
  missing as its participants change - before `keep_participants` makes it again -; no ordinary
  path leads there, and the maintainer accepted it rather than an exception for the own group.
  Stored data need not be deleted.
