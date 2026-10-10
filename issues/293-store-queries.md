# 293 — A store list takes no condition and no limit

**Status:** in progress
**Area:** Persistence

## The problem

The `list_*` methods of the stores take nothing but the caller's context: every list is all
entities of a kind the caller may read, unsorted, unbounded. Whatever narrows it - those of a
scope, those published in a channel, those shared with someone, the first fifty - is done by the
caller after everything was read, and the policy is asked of every entity on the way (issue
[292](292-store-lists-read-everything.md)). A database store ([03](03-postgres-entity-store.md))
could answer such a question with an index, but is never asked one.

## Why it matters

- Lists that read everything do not serve a deployment that keeps much - artifacts first, then
  sessions, workspaces, skills.
- Every narrowing written as code at its caller is a narrowing a database cannot take over; one
  written as a declarative condition can be pushed into a `WHERE` clause.
- Paging - a history in pieces ([218](218-paged-histories.md)), a list of artifacts - needs a limit
  and a stable order the store keeps.

## Considerations

**The query.** Every `list_*` takes a query of its kind:

```rust
Query<F: Field> { filter: Option<Filter<F>>, order: Order<F>, limit: Option<u32>, after: Option<Cursor> }

enum Filter<F> {
    Eq(F, Value), In(F, Vec<Value>), IsNull(F),
    And(Vec<Filter<F>>), Or(Vec<Filter<F>>), Not(Box<Filter<F>>),
    Any(Collection, Box<Filter<..>>),   // a version published in channel X, an entry naming one of these subjects
}
```

- **Fields typed per kind** (`ArtifactField::Owner`, `Scope`, `VersionChannel`, ...), not strings:
  the compiler checks a query, and every kind says what may be asked of it. A trait `Field` gives
  `value_of(&entity)` for evaluating in memory and the column for SQL.
- **Purely declarative**: no functions, no patterns, no text search - everything translates into a
  `WHERE` clause; `Any` into `EXISTS` or a JSONB operator.
- **Paging by cursor**, not by offset, over a stable order (the id, or a time with the id).
- **The memory and JSON stores** evaluate a query in memory: still a scan, but without a policy
  question and a membership read per entity. Indexes are for a database store, or for a memory
  store once data grows.

**Access, the hard part.** As long as the policy decides afterwards in code
(`SecurityManager::check`), a limit cannot be kept: of fifty matches the policy may drop forty. Two
phases:

1. **The query without access.** The store applies the query, then the policy as now; a limit is
   met by reading on until enough readable entities are found. Correct, no change to the policy,
   and the narrowing by scope, place and subject is pushed down already.
2. **Access as a condition.** The policy states what a context may read as a filter of its own -
   for the ownership policy, allow-only as it is (issue 23):
   owner = P ∨ an entry names P ∨ an entry names a role of P's ∨ an entry names a place P is a
   member of ∨ the system's. That needs the subjects of a principal: their roles, and the places
   they are a member of (`Memberships::places_of`, itself a query over participants). The rule then
   lives in two forms - `check` for one entity, the filter for lists -, held together by a test that
   the filter matches exactly what `check` grants, over generated entities. An external policy
   ([24](24-opa-security-manager.md)) answers the same by partial evaluation.

**Clearances** - a classification against a user's roles, with compartments and a read relation -
are harder to state as a condition; they stay a check afterwards at first, reading a little more,
since they rarely drop anything.

**What it takes from 292**: 292's step 2 - artifacts by place and by subject - becomes queries of
this kind instead of methods of their own. 292 keeps its step 1, the reading into the views.

**What it costs**: a parameter on every `list_*`, a field enum per kind, an evaluator in each
store, and in phase 2 a second form of the policy's rule. An L - but one mechanism for every list,
instead of a method per question.

## Done when

- Every `list_*` takes a query - filter, order, limit, cursor -, the memory and JSON stores
  evaluate it, and the store suite covers it; the artifacts of a channel and the user's list are
  queries.
- Every caller of a `list_*` that narrows what it read asks it in its query, wherever the filter
  can say it; what stays code at the caller says why.
- The access of a context is part of the query (phase 2), and a test shows the policy's filter and
  its `check` agree.
- STORAGE.md describes queries; SECURITY.md how access becomes a condition.

## History

- **2026-10-10** — Opened at the maintainer's request, discussing issue 292: a generic, purely
  declarative condition and a limit for every list, so that a database can take them over.
- **2026-10-10** — Phase 1 done, branch `issue-293-store-queries`, decided with the maintainer
  before it was built: fields are typed per kind and name a path into the document as it is
  serialized, values are JSON (`Filter::eq`, `Filter::is_in` serialize them); every `list_*` of
  the entity and component stores takes a `Query` and answers a `Page` - only artifacts have
  fields yet (`ArtifactField`), every other kind `NoField`; the order is that of the keys, the ids,
  and the cursor is the last key, opaque to whoever holds it. Built:
  - `store::query`: `Field`, `NoField`, `Filter` with `matches` on a JSON document, `Query`,
    `Cursor`, `Page`. The page is made in one place for both stores (`Access::page`): in the order
    of keys, after the cursor, the filter, then the policy, until the limit; the per-entity checks
    replace the list filters (`Access::readable`, `readable_deleted`). The memory store judges
    each entity serialized, the JSON store each document as JSON before reading it as its type.
  - A page that goes on is full; the one after it may be empty, where nothing that followed was
    the caller's to read - finding that out beforehand would cost a policy question per entity left.
  - The artifacts of a channel are a query (a version published in it), and the user's list too
    (their own, and those whose list or a version's names them or a role of theirs). That changed
    what `GET /artifacts` answers, as agreed: what a user reads only as a member of a channel is
    listed by that channel, no longer among their own.
  - Every other caller asks `Query::all()` and keeps its `Vec` (`.map(|page| page.items)`).
  - The store suite runs queries on both stores (`queries_behave`); a test shows a limit counting
    what the caller may read; the security suite shows the user's list and the channel's.
  Not yet: limit and cursor over REST - for the lists that will want them, 218 and 291 -, ordering
  by another field, and phase 2.
- **2026-10-10** — Widened at the maintainer's request, on the same branch: every caller of a
  `list_*` was checked for a narrowing it does in code after reading, and asks it in its query
  where the filter can say it. Built:
  - Fields for the other kinds: what lies in a scope shares `ScopeField` - its scope, and the
    session of a channel's scope - for personalities, session templates, agents, workspaces and
    skills, one type for five kinds, since they are asked the same; artifacts have the same two
    among theirs (`ScopedField` names them for any kind). Channels have their session
    (`ChannelField`), sessions their participants (`SessionField`).
  - The scope makes its conditions: `Scope::within` - what lies in it or within it, as `contains`
    says - and `Scope::seen_from` - what is found from a place by a user, as the lookup by name
    finds it; a test holds both to their rule over every pair of scopes.
  - Queries instead of filters in code: what goes with a session or a channel when it is deleted,
    restored or purged (`scoped`, and the managers' `purge_lying_in`), the channels of a purged
    session, the sessions an agent takes part in when it is deleted, the deleted channels of a
    session (`SessionView::deleted_channels`), the skills found from a place (`usable_skills`).
  - Left in code, each for a reason: what was deleted since a time (the filter compares no times
    yet), lookups by name (names compare normalized, issue
    [250](250-unique-names-per-name-space.md)), and the clearance of a viewer in the views'
    lists - that belongs to phase 2. Templates looked up by name in the REST server and the TUI
    are lookups by name.
  - The store suite runs queries by scope, by a channel's session and by a participant on both
    stores.
