# 332 — The policy answers entity by entity, so no database can filter by it

**Status:** open
**Area:** Identity and access management

## The problem

A store learns who may read an entity only by asking the `SecurityManager` about that one entity
(`SecurityManager::check` with `ActionType::Entity`) and the `ConfidentialityPolicy` about how it is
classified (`may_reach`). Listing therefore reads candidates and filters them afterwards:
`Access::page` in `caelumora-model/src/store/access.rs` loads one after the other, asks about each,
and reads on until the page is full. The conditions of a query (issue
[293](293-store-queries.md)) are declarative and could be pushed into a database's `WHERE` clause;
the condition of who may read could not.

In memory and in the JSON files that costs nothing worth naming. In a database - Postgres, a table
or more per kind, partly with queries written by hand - it would mean reading thousands of rows to
answer a page of fifty that the caller may see, and a join written by hand could not be limited to
what the caller may reach at all.

## Why it matters

- A database backend is the condition for issues [21](21-scaling.md) and
  [122](122-several-instances.md); with the check after reading, its lists do not scale with what a
  user may see but with what exists.
- Once kinds come from plugins (issue [323](323-kinds-enumerated-in-the-core.md)), a plugin with
  tables and queries of its own needs a way to restrict them to what the caller may reach without
  knowing the policy, and without writing the rules of `access.rs` once more.
- A wrapping store that filters what another one read (a "secured store" over a raw table) was
  considered while discussing issue 323 and rejected for this reason: it can only filter after
  reading.

## Considerations

- **The policy as a predicate.** For the verbs on entities, the policy gives a condition over the
  security attributes - which every kind has alike (`Secured::security_attributes`: `owner_id`,
  `acl`) - and how it is classified, for a context and a verb. A store evaluates it in memory or
  translates it, as it does the `Filter` of a query already. For `OwnershipSecurityManager` and
  reading: owned by the system or nobody, or by the principal (or the caller, issue 264), or an
  entry of the access list covering `Read` names the principal, one of their groups or one of their
  roles; and the clearance holds. For `allow_all`: true.
- **One source of the rule.** If the policy keeps `check` beside the predicate, the two can come
  apart. Cleaner: for `ActionType::Entity`, `check` evaluates the predicate in memory; the other
  actions (two sets of attributes, authors - `ApproveToolCall`, `ModifyMessage`, …) stay as they
  are, since no list is filtered by them. The security suite could check that both agree.
- **Memberships turn round.** The policy asks `members_of(group)` today. A predicate needs the
  groups of the principal - best in the `SecurityContext`, as the roles are (issue
  [304](304-review-of-the-permission-concept.md): what a decision needs to know of the subject is
  in the context). Overlaps with issue [287](287-memberships-cache.md).
- **Single operations** become `UPDATE … WHERE id = $1 AND <may change>`; that a change is checked
  against the attributes as stored holds by itself. No row changed is then told apart as "not found"
  or "refused" by the reading predicate, so that `Enforcement::require` still records a refusal.
  Filtering a list stays a question, as it is today.
- **Names** unique in their scope (issue 250) are a unique index in a database; `name_free` stays
  the form in memory.
- **What a kind adds.** An artifact is reached also through the access lists of its versions (issue
  [316](316-store-checks-artifact-without-versions.md)); the predicate has to be able to say so, or
  that rule stays outside the store.
- **Tables of plugins.** With the attributes alike in every kind, a database backend can lay out and
  translate the security columns once, also for a table a plugin writes by hand: the plugin names
  its table and columns and appends the condition the backend translates, without knowing the
  policy.

## Done when

- The policy gives, for every verb on entities, a condition that a store can evaluate in memory and
  a database can translate, and `check` on an entity is decided by the same condition.
- The stores in memory and in JSON files filter by it; `Access::page` no longer asks entity by
  entity.
- What a decision needs of the subject - roles, groups, clearance - is in the `SecurityContext`.
- The security suite shows that the condition and the single decision agree.
- `docs/SECURITY.md` and `docs/STORAGE.md` describe it.

## History

- **2026-10-12** — Opened while discussing the stores of plugins for issue 323: with a Postgres
  store, the conditions of security would be a `WHERE` clause, so a wrapping store that filters
  after reading is no way, and the policy has to give a predicate that every store implements.
  Not needed before a database backend.
