# 332 — The policy answers entity by entity, so no database can filter by it

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Identity and access management
**Summary:** The policy decides entity by entity, so a store filters a list after reading it and no database could put who may read into its query.

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

## Decision

Access lists are row-level security - an aspect of their own beside a query, not a condition in
it -, and a store is told them as such, in a form that says what they mean, not as a `Filter`.

1.  **The policy gives an `Admission`** (`caelumora-model`, `security`): which entities a caller is
    admitted to by their security attributes, in the words of `SecurityAttributes` -

    ```rust
    pub struct Admission {
        /// Who is admitted as themselves - by owning it, by an entry naming them or a group of
        /// theirs: the principal, and the caller where it takes part itself (issue 264).
        pub actors: Vec<ActorId>,
        /// Whether what the system or nobody owns admits as well.
        pub unowned: bool,
        /// What an entry of the access list must cover to admit; `None` where owning alone does.
        pub grant: Option<Grant>,
        /// The roles an entry may name (those that count, `SecurityContext::has_role`).
        pub roles: Vec<RoleId>,
        /// Whether an entry for everyone admits.
        pub everyone: bool,
        /// Whether an entry for a group admits the members it had when it was deleted.
        pub deleted_groups: bool,
    }
    ```

    with `Admission::admits(attributes, memberships)`, and `AccessPolicy::admission(context, kind,
    verb) -> Option<Admission>` beside `check`, `None` by default: where a policy cannot say its
    rule so, a list is filtered entity by entity, as today. A request is made in the name of one
    context, which stands for one actor or two - the principal and the caller - and for several
    roles; the grants that cover the one needed are the model's rule (`Grant::covers`, offered as
    `Grant::covering`), the roles that count the context's (`has_role`), so neither is listed by
    the policy. For the `OwnershipPolicy`, reading as a user is `actors = [principal]`, `unowned`,
    `grant = Some(Read)`, the roles that count and `everyone`; as an agent the owner and the agent,
    with `everyone` by its owner; what was deleted (`ReadDeleted`) the principal alone,
    `grant = None`.
2.  **One rule.** `check` decides the verbs on entities by the admission it gives
    (`admission(..)?.admits(..)`), so that the condition a store applies and the single decision
    cannot come apart; the other actions - two sets of attributes, authors - stay as they are.
3.  **A deleted group admits nobody**, but where a deletion is told. Its access entries grant
    nothing once it is deleted. The groups of a session's participants go with it, before its
    deletion is announced, and the event bus asks the policy later still, as it delivers; so who
    is told of a deletion is asked by a verb of its own, `HearDeleted`, whose admission is
    reading's with the flag `deleted_groups`: an entry for a group admits the members it had when
    it was deleted. `Memberships` answers the group with its members and whether it was deleted
    (`Memberships::group`); the admission decides whether that counts.
4.  **The groups stay with the store**, not in the `SecurityContext`: the groups of an actor are
    unbounded - one per session and channel they take part in -, and a store resolves membership
    where the groups are kept, a database by a join. This reverses the consideration "Memberships
    turn round" above, and settles the point 295 left open in its favour.
5.  **The storage is told the attributes.** A `Document` carries its `security_attributes`, as it
    carries its name and its mark of deletion - what a storage needs to know of it without reading
    it as its type -, and `DocumentStorage::scan` takes the caller's admission beside the filter of
    the query: two aspects, not joined into one filter. A scan may hand out more than it admits,
    never less; `CheckedStore::page` asks for the admission through `Access` and still decides
    every candidate as today (`Access::readable`), now only those the storage handed out.
6.  **Each storage applies it as it can.** In memory, by the document's attributes - owner,
    actors, roles, everyone -, an entry for a group counting as admitting, since a storage of one
    kind does not read the groups; the decision after the scan settles it. The JSON files leave it
    to the store, as they leave the mark of a deletion: they know the attributes only of a body
    read as its type, which the store does next.
    In Postgres, by a table of the access entries per kind, `entity_<kind>_access` - the owner and
    every entry of the access list as a row `(subject, relation, entity_id)`, written from the
    document's attributes in the transaction that writes it and removed with it -, and the
    admission as a semi-join over it and the groups an actor belongs to (`entity_group`). Who is
    everyone, and what nobody or the system owns, have no UUID: the Postgres store names them as
    values of their own.
7.  **What is classified** stays decided entity by entity (`may_reach`): it narrows what the access
    list let in and was nowhere the cost measured. Should it be needed, it becomes a second part of
    the same aspect, not a filter.
8.  **Lists only.** Reading, changing and deleting one entity stay as they are.
9.  **Issue [455](455-components-configuration-kept-as-entities.md) comes before the Postgres
    step**, so that every kind is keyed by a UUID - the table of a kind, of its access entries and
    of the groups joined by a `uuid` instead of a text.

### What was weighed and not done

- **A `Filter` over the security fields, with a reference to another kind** (`Filter::Refers`, a
  group whose members contain the actor), judged in memory and translated by `Condition`: general,
  but the store loses what the condition means. Translated as it reads - a correlated `EXISTS` per
  row - it walks the table of the kind; the form that scales would have to be recognised in it
  again. It would also have needed fields of the security attributes by path in every body, a
  judgement of three values in `matches`, and a reference only loosely judged in memory.
- **The groups of the principal in the `SecurityContext`**, listed into the query: unbounded.
- **The access list as an array column** of the table of a kind (`jsonb[]`, or a composite of a
  `uuid` and a grant), compared by `&&` with the entries of the caller and of their groups: fast
  for whoever sees little, but compared row by row with every group of an actor, so slow for one
  in many groups.
- **Containment on the body's GIN index**: no statistics to choose a plan by, and no join under an
  `OR`.

### Measured

On the test database (Postgres 17): 100 000 private sessions, each with a group of its
participants - 100 000 groups -, users skewed; a page of 100, the best of three runs, prepared and
generic plans alike.

| reader | today: body, walked by id | access list as array | access table |
|---|---|---|---|
| in 101 groups, sees 0.1 % | 375 ms | 0.8 ms | 0.3 ms |
| in 177 groups | 86 ms | 1.3 ms | 0.6 ms |
| sees nothing | 158 ms | 0.0 ms | 0.0 ms |
| in 30 160 groups, sees 30 % | 0.7 ms | 590 ms | 41-67 ms |

With 5 % of the sessions for everyone, and on templates 90 % of which the system owns, every form
answered in under 7 ms. With the access table, the planner chose by its statistics - a nested loop
for a reader who sees little, a merge join for one in many groups; the groups found by a column of
their members (`uuid[]`, GIN) took 67 ms, by a table of memberships 41 ms.

## Steps

1.  **The model.** `Admission`, `AccessPolicy::admission`, the roles that count of a context; the
    `OwnershipPolicy` gives its admission and decides `check` on entities by it; a deleted group
    admits nobody, but where a deletion is told (`HearDeleted`), which the event bus asks.
2.  **The stores.** `Document` with its security attributes, `DocumentStorage::scan` with the
    admission, `CheckedStore::page` asking for it; memory applies it, the JSON files leave it to the
    store; a policy wrapping another - the recorder of the security suite - gives the admission of
    the one it wraps.
3.  **Postgres** (after 455). The table of the access entries per kind - a new form of the tables of
    the kinds -, the admission as a semi-join, `Grant::covering` for the grants it names, a test
    that what the database admits is what `check` decides.

## Done when

- The policy gives, for the verbs on entities, an admission that a store applies and a database
  translates, and `check` on an entity is decided by it.
- A list hands the storage the admission, and the stores in memory and in Postgres apply it; what
  the storage hands out is still decided entity by entity.
- A test of the security suite shows that what each store admits is what `check` decides.
- `docs/SECURITY.md`, `docs/PERMISSION-CHECKS.md` and `docs/STORAGE.md` describe it.

## History

- **2026-10-12** — Opened while discussing the stores of plugins for issue 323: with a Postgres
  store, the conditions of security would be a `WHERE` clause, so a wrapping store that filters
  after reading is no way, and the policy has to give a predicate that every store implements.
  Not needed before a database backend.
- **2026-10-15** — Issue [295](295-access-as-a-query-condition.md), which asked for the same
  change, was rejected as a duplicate of this one. Its considerations, discussed with the
  maintainer on 2026-10-10, belong to this issue: the condition comes from the policy, `None`
  where it does not restrict up front; it is evaluated per document over the security attributes,
  written over a field type of their own rather than each kind's fields; clearances stay a check
  afterwards. Two points there differ from the considerations above and are to be settled here:
  295 keeps `check` per entity as a second safeguard with a test holding both forms together,
  where this issue decides `check` by the same condition; and 295 resolves membership as a join in
  the store, where this issue puts the principal's groups into the `SecurityContext`.
- **2026-10-23** — Decided with the maintainer: access lists as row-level security, given to the
  store as an `Admission` beside the query rather than as a filter; `check` decided by it; the
  groups resolved by the store, a deleted group admitting nobody; in Postgres a table of access
  entries per kind, chosen by measurement, after issue
  [455](455-components-configuration-kept-as-entities.md). Status in progress, priority medium:
  the condition of a Postgres store that scales.
- **2026-10-23** — Decided with the maintainer, amending point 3: the participants of a session
  reach it by its group, which is deleted before the deletion is announced and asked about after,
  as the bus delivers - so "a deleted group admits nobody" alone would tell them nothing. Who is
  told of a deletion is asked by a verb of its own, `HearDeleted`, admitted as reading with
  `deleted_groups`. Point 1 said an agent reads without `everyone`, which was wrong and is
  corrected there: an entry for everyone admits an agent by its owner, as it did, and still does.
- **2026-10-23** — Step 1: `Admission` in the module `security::admission`, given by
  `AccessPolicy::admission` - `None` by default and for the system - and deciding every verb on
  entities of the `OwnershipPolicy`, whose other actions are decided by an admission of reading or
  owning as well; `SecurityContext::counting_roles`; `Memberships::group` in place of
  `members_of`, answering the group and whether it was deleted; `Verb::HearDeleted`, asked by the
  event bus of a deletion. A deleted team grants nothing until it is restored. What a deleted
  session or channel announces as it stops reaches its owner alone, since it is stopped only once
  its groups are gone - accepted, the order left to issue [458](458-deleted-while-it-runs.md).
- **2026-10-23** — Step 2: a `Document` carries its `security_attributes`, `DocumentStorage::scan`
  takes the admission beside the filter, and `CheckedStore::page` asks for it through `Access` - of
  `Get`, of `ReadDeleted` for the trash -, still deciding every candidate. Memory narrows by
  `Admission::may_admit`, an entry for a group letting a document through. The JSON files leave the
  admission to the store, as they leave the mark of a deletion: the attributes are known only of a
  body read as its type, which the store does next anyway, so narrowing there would read every file
  as its type once more, the admitted ones twice - point 6 and the steps amended so. Postgres hands
  out what it did until step 3. The `Recorder` of the security suite gives the admission of the
  policy it wraps. The store suite holds every backend to what `check` decides
  (`lists_admit_what_check_decides`).
- **2026-10-23** — Step 3: the tables of the kinds keep the access entries of their documents
  beside them, `entity_<kind>_access`, written in the transaction of each write - a new form of the
  tables, 3 -, and a scan narrows by the admission as a semi-join over them and over the groups an
  actor belongs to (`Admitted`). The groups are found by containment of the members in the bodies
  of `entity_group` (`Group::with_member`), not by a table of memberships: measured on 100 000
  documents and groups, a reader in 30 000 groups took 66 ms that way, 43 ms by such a table, every
  other reader about 1 ms either way; a table of memberships would have been a special case of the
  storage of the groups, and laying kinds out in forms of their own is issue
  [460](460-kinds-kept-in-one-form-in-postgres.md). `Grant::covering` gives the relations an entry
  must have. Done: what each store hands out is what `check` decides, held by the store suite for
  memory, the JSON files and Postgres.
- **2026-10-23** — Afterwards, at the maintainer's word: `Group::with_member`, which step 3 added,
  and `Group::teams` read as questions to a group but make a condition of a list; they became
  constructors of the filter, `Filter::with_member` and `Filter::teams` beside `GroupField`, as a
  history's conditions are (`Filter::said_by`); the scope's `Scope::within` and `Scope::seen_from`
  became `Filter::lying_within` and `Filter::seen_from` alike, `Filter::within` being the range of a
  number already. `docs/CODING-GUIDELINES.md` says so as a rule.
