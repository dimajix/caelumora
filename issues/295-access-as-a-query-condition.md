# 295 — A list asks the policy entity by entity, so no database can take access over

**Status:** open
**Area:** Persistence

## The problem

Since issue [293](293-store-queries.md), a store's list takes a query: a condition, a limit and a
cursor. Who may read what is not part of it. `Access::page` reads each entity that meets the
condition and asks the policy about it (`SecurityManager::check`), one at a time, until the page is
full. That is correct and the limit is kept, but:

- A database store ([03](03-postgres-entity-store.md)) cannot push access into its `WHERE` clause.
  It reads every entity that meets the condition, including those the caller may not read, and
  hands each to the policy.
- The cursor is not exact. A full page says more follows when keys remain, even if the caller may
  read none of them, so the page after it may be empty.

This was phase 2 of issue 293. It was split off because the memory and JSON stores gain little from
it: they read each document anyway, and `check` costs the same lookups a condition would. It pays
off once a store can execute a condition itself.

## Why it matters

- A Postgres store that filters access in code reads as much as the stores do now. For a deployment
  that keeps much and shares little, that is most of what it reads.
- Paging with a cursor ([218](218-paged-histories.md)) should not hand out empty pages.

## Considerations

These were discussed with the maintainer on 2026-10-10, while closing phase 1 of issue 293.

**The condition comes from the policy.** Only the policy knows how each kind of entry in an access
list is tested. A role may be narrowed by an agent's personality. A member counts even in a place
that was deleted. A store that compared access lists with the context itself would need its own
copy of the ownership rules. Those rules would sit with the wrong owner, and with an external
policy ([24](24-opa-security-manager.md)) they would be wrong.

So `SecurityManager` gets a method that states what a context may read as a condition. It returns
`None` when the policy does not restrict up front: `DummySecurityManager`, a `System` principal,
and the policies made for tests, through the trait's default. An external policy answers by partial
evaluation, or with `None`.

**The condition is evaluated per document.** Every object has its own access list, and the
condition is not a statement about any one of them. It is a predicate the store evaluates on each
document, with that document's data, the way row-level security works in Postgres.
`Any(Acl, …)` walks through the entries of the object at hand. For the ownership policy it reads:
the owner is the system or nobody, or one of the acting actors (the principal, and the caller where
it may read for itself, issue 264), or some entry grants what covers reading and names one of
these actors, one of the roles that count for the caller, or a place one of the actors is a member
of.

**Fields relative to the security attributes.** The policy decides by `SecurityAttributes` alone.
Every protected kind keeps them in the same form under `security_attributes`. So the condition is
written over a field type of its own, relative to that object: owner, access list, and within an
entry its subject and grant. The store applies it to `document["security_attributes"]`, joined by
"and" with the query's own filter. Only the store knows where the attributes lie.

The alternative was a trait like `ScopedField` that every field enum implements. It was not chosen,
for three reasons:

- `SecurityManager` is used as `dyn SecurityManager`, and a generic method returning `Filter<F>`
  cannot be called on it.
- Nine kinds would repeat the same four fields.
- Unlike a scope's condition, the access condition is never combined by a caller, so one shared
  filter tree gains nothing.

One caller does need the condition inside a kind's fields: `Views::artifacts`, over the access
lists of the versions. It can keep its own condition in the system's name until a way exists to
nest an access condition under a field of a kind.

**Membership is a join.** Whether an actor is a member of the place an entry names lies in another
document, the participants of that session or channel. A filter over one document cannot answer it.
These options were weighed:

- **The caller's places up front** (`Memberships::places_of`), compared with `In(Subject, …)`. Not
  chosen. The list can be long, and it would have to include deleted places, since membership
  counts in them too. In the memory and JSON stores it reads every session and channel.
- **A join in the filter** (recommended). An operator that says "the place this field names has one
  of these actors as a participant", which the store resolves. The memory and JSON stores read only
  the places that entries actually name, cached for the query. Postgres turns it into `EXISTS`. Who
  counts as a member already lives with the store (`StoreMemberships`), and the operator would use
  that rule rather than copy it.
- **A coarse condition.** It lets through every entity with any entry for a place that grants
  reading, and leaves the rest to the check afterwards. Cheap, but the inexactness stays for
  everything shared by membership.

**The check per entity stays** as a second safeguard. Where the condition and `check` agree, it
drops nothing. Where a policy states no condition, it keeps lists correct.

**The rule lives in two forms**, `check` for one entity and the condition for lists. A test holds
them together: over generated owners, access lists, grants, roles and memberships, the condition
holds exactly when `check` allows.

**Clearances** stay a check afterwards, as issue 293 already decided.

## Done when

- A store that can execute a condition exists ([03](03-postgres-entity-store.md)), or one of the
  existing stores needs it.
- The policy states what a context may read as a condition, the stores apply it before they read an
  entity, and membership is resolved by the store without listing the caller's places.
- A test shows the ownership policy's condition and its `check` agree.
- SECURITY.md describes how access becomes a condition; STORAGE.md how a store applies it.

## History

- **2026-10-10** — Opened at the maintainer's request, split off from phase 2 of issue
  [293](293-store-queries.md). The considerations above record the discussion. The work waits for a
  store that can execute a condition.
