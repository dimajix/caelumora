# 460 — Every kind is kept in one form in Postgres, so none can be laid out for how it is reached

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Persistence
**Summary:** The Postgres store keeps every kind alike - a table of id, name, mark and body, searched by a GIN index of the body -, so a kind that is reached in a way of its own, such as the groups by their members, cannot have a table or column of its own that answers that faster.

## The problem

The Postgres store (issue [03](03-postgres-entity-store.md)) keeps the documents of every kind in a
table of one form, `entity_<kind>`, written by one generic storage (`PgDocuments`) that knows no
kind's type: what a query asks of a body is translated over its JSON (`Condition`), answered by a
GIN index of the body where it can be asked as containment. That holds for a plugin's kind as for
the core's, and is what keeps the store free of special cases.

Some kinds are reached in a way of their own, often and over many entities, and the one form
answers that more slowly than a form made for it would:

- **The groups by their members.** Narrowing a list by who may read it (issue
  [332](332-policy-as-a-predicate.md)) finds the groups an actor belongs to by containment in the
  bodies of `entity_group`. Measured on 100 000 groups, for a reader in 30 000 of them: 83 ms by the
  body, 56 ms by a column of the members (`uuid[]`, GIN), 43 ms by a table of the memberships
  `(actor, group_id)`; for every other reader under a millisecond either way. 332 took the body,
  since a table of memberships would have to be written by the storage of the groups alone - a
  special case of the generic storage.
- **More of the same kind** can be expected: the versions of an artifact with their own access
  lists (issue [316](316-store-checks-artifact-without-versions.md)), what lies in a scope, looked
  up by its holder, the participants of a session or channel.

## Why it matters

The scaling of the stores (issues [21](21-scaling.md), [122](122-several-instances.md)) is decided
by how the frequent questions are answered. A form of one kind's own is the usual answer in a
database; without a way to have it, each such question either stays slower or is built into the
generic storage as an exception.

## Considerations

- **What a form of its own needs.** Tables or columns beside the kind's table, made with it and of
  a version of their own; written in the transaction that writes a document, from what the
  document says; read by the conditions a store translates - a filter of the kind, or a part of an
  admission (`Subject::Group` resolved by the memberships instead of the bodies).
- **Declared by the kind, laid out by the backend.** A kind names what it is reached by - a field
  kept as a set of relations (`GroupField::Member`), a field kept as a column -, in the model, apart
  from any backend (`Stored`, beside `unique_name`); each backend lays it out as it can: Postgres
  as a side table or a generated column with its index, memory and the JSON files not at all. A
  plugin's kind declares the same, without depending on Postgres. Translating a filter then looks
  for a declared field first.
- **Written by a layout of the backend's, per kind.** The Postgres store holds a layout for a kind -
  its extra tables, how a document fills them, how a condition reads them -, registered for the
  core's kinds in the Postgres crate, and by a plugin for its own through an extension the Postgres
  crate offers. More freedom, but a plugin with a layout depends on the Postgres crate, and every
  layout is code of its own to keep in step with the kind.
- **What stays.** The table of the kind keeps the body whole, so that a document reads without the
  extra tables, and the store judges every document again (issue
  [447](447-store-seam-not-fit-for-a-database.md)): a form of its own only narrows what is read,
  it never decides. So a layout that is out of step with a document costs a slower answer at most,
  not a wrong one - as long as it never hands out less.

## Done when

- A kind can be kept in Postgres in a form of its own for how it is reached, without a special
  case of the generic storage, and a plugin's kind can be too.
- The memberships of the groups are kept so, and the admission of 332 finds an actor's groups by
  them.
- `docs/STORAGE.md` describes how a kind declares, or a backend lays out, such a form.

## History

- **2026-10-23** — Opened while implementing step 3 of [332](332-policy-as-a-predicate.md), which
  finds an actor's groups by containment in the groups' bodies; a table of memberships answered a
  reader in many groups in half the time, but would have been a special case of the generic
  storage. The maintainer asked for the wider question: forms of their own for kinds, in Postgres,
  to make how they are reached faster.
