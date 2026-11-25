# 605 — A personality's limit means one thing for roles and another for clearances

**Status:** in progress
**Type:** question
**Priority:** medium
**Area:** Identity and access management
**Summary:** The roles a personality limits its agents to narrow the owner's roles where a role is matched - only those the owner holds and the limit names count - but bound the clearance as a ceiling of their own, which clears beside the owner's roles whether the owner holds them or not; one concept has two meanings, and a rule built on one of them misses the other.

## The problem

A personality's `roles` (`Personality::limiting_roles`, issue 179) reach the agent's context as
`SecurityContext::limiting_roles`, and two kinds of questions read them differently:

- **Roles that count** - `has_role`, `counting_roles`, as an access list's `Role` entry is matched
  (`OwnershipPolicy`, `Admission::roles`): the owner's roles that the limit names. A role the owner
  does not hold counts for nobody.
- **Clearance** - `ConfidentialityQuestions::cleared_for`
  (`caelumora-security/src/confidentiality.rs`) and `ResolvedParticipant::clearances`
  (`caelumora-kernel/src/channel/resolved.rs`): the owner's roles must clear, and the limit's
  roles must clear by themselves, held by the owner or not
  ([CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md), "an agent is cleared only if its owner is *and*
  those roles are").

So the limit is a set of roles where a role is matched, and a ceiling ("at most what `staff`
sees") where a clearance is judged. The seed's `hermes-remote` relies on the ceiling: limited to
`[staff]`, its agents are cleared up to what `staff` clears for an owner who holds a higher role and
not `staff` itself, while no `Role(staff)` entry of an access list admits them.

## Why it matters

- **Concepts are named:** one field, two concepts, and nothing names which a question asks.
- **Rules built on one meaning miss the other.** Issue
  [597](597-sub-agents-are-bounded-by-a-list-of-personalities-alone.md) first compared the limits
  of a sub-agent and its starter by the roles that count, which let a sub-agent be cleared higher
  than its starter; it compares the lists now (`SecurityContext::covers_limit`), which is stricter
  than either meaning needs.

## Considerations

- **One meaning, the roles that count, for the clearance too.** `cleared_for` and `clearances`
  would clear by `counting_roles`. A limit stops being a ceiling: an owner must hold a role the
  limit names for it to count, so a shared personality has to list every role its users may hold -
  `hermes-remote` would clear an owner who holds `admin` and not `staff` for nothing but what is
  public. It only narrows; no agent is cleared for more.
- **One meaning, the ceiling, for the roles too.** A `Role` entry would admit an agent by a role
  the limit names that its owner does not hold - a widening the rules forbid (SECURITY.md, an agent
  never does more than its owner).
- **Two concepts, named apart.** The ceiling and the role filter as two fields of a personality, or
  the one field documented as both, with each question saying which it asks.

## Decision

Decided by the maintainer on 2026-11-25: **a limit is a ceiling in every question** - a fourth
option beside those above. Every question that roles decide is asked once by the owner's roles and
once by the roles of the limit, and both have to allow: an agent may at most what its owner may,
and at most what the roles of its limit may, whether its owner holds them or not.

- **Clearance** stays as it is: `cleared_for` and `ResolvedParticipant::clearances` ask so already.
- **Roles in an access list** are asked the same way: an entry covering the grant has to name one
  of the owner's roles, and one - the same entry or another - a role of the limit. Whoever holds
  `head` reads the accounts by an entry for `head`; their agent limited to `accountant` reads them
  where an entry for `accountant` grants it as well - its owner need not hold `accountant` -, and
  not what only `head` is granted.
- **No widening:** the owner's question is asked as before, so an agent never does more than its
  owner. Option 2 above reads the limit as taking the place of the owner's roles; asked beside
  them, it widens nothing.
- **Rejected:** one meaning, the roles that count, for the clearance too (option 1). It reads a
  limit as roles the owner must hold, which presumes an order among roles that Caelumora does not
  have: an owner with a higher role and without the lower one would get nothing from an agent
  limited to the lower. A hierarchy of roles to repair that would have a higher role inherit every
  right of the lower, everywhere, which does not hold in general - a head of finance need not
  book. Rejected too: two fields (option 3), the same behaviour as two concepts.
- **The model** is the common one of delegation: what the delegate may is what the delegator may,
  cut by a bound of the delegate's own - an OAuth client's scopes against its user's rights, an
  AWS permissions boundary, the restricting SIDs of a Windows restricted token, which are checked
  against the same access list in a second pass. Narrower than the last: the limit is asked only
  where roles decide. An entry for the owner, a group of theirs or everyone, and owning, admit the
  agent as they admit its owner - as they did before; a second pass of the restricting SIDs would
  refuse them unless the limit named them.
- **Corrected:** the seed does not rely on the ceiling, as "The problem" says. Every user of
  `examples/config.yaml` and `examples/users.yaml` holds `staff`, so for `hermes-remote` both
  meanings agree there. No test held a limit naming a role the owner does not hold.
- **`covers_limit` stays as it is:** a personality whose limit names no role the starter's does
  not asks its second question by fewer roles, so its agent is admitted and cleared for nothing
  the starter is not. Only its reasoning changes.

## Plan

One step, a change of behaviour with no restructuring before it. The pattern is the clearance's -
two questions, both of which have to allow (`cleared_for`) -, carried over to `Admission`.

```rust
// caelumora-security/src/admission.rs
pub struct Admission {
    /// The roles an entry may name: the owner's (`SecurityContext::roles`).
    pub roles: Vec<RoleId>,
    /// The roles the caller is limited to, if any (`SecurityContext::limiting_roles`): where
    /// set, an entry covering the grant has to name one of them as well - the same entry as
    /// for `roles`, or another (issue 605).
    pub limiting_roles: Option<Vec<RoleId>>,
    // the other fields unchanged
}
```

- `SecurityContext::has_role` and `counting_roles` are removed: no question asks by the roles that
  count any more. The tests that call them (`controller/agent.rs`, `controller/manager.rs`) read
  the context's fields.
- `OwnershipPolicy::granting` gives the owner's roles and the limit.
- `Views::subjects` names the viewer by the owner's roles - for a limited viewer more than admits
  it, which the store decides as it lists; the artifacts' list is its one user. The copy of
  `counting_roles` written out there goes with it.
- The Postgres store (`Admitted::of`) narrows by the owner's roles alone: for a limited caller its
  scan hands out more, which the store judges again, as `DocumentStorage::scan` allows. An exact
  condition - the entries of the limit's roles intersected with those of the owner's - would cost
  a second sub-query and its parameters for shorter scans of limited callers alone; not proposed.

Tests:

- `admission.rs`: two entries admit together; an entry for one of the owner's roles alone does
  not, nor one for a role of the limit alone.
- `ownership.rs`, `a_role_grants_to_whoever_holds_it_an_agent_narrowed_by_its_personality`: the
  agent reads what its owner's role and its limit's role are both granted.
- `confidentiality.rs`: the ceiling - an owner whose role clears for `confidential`, limited to a
  role that clears for `internal` which they do not hold, is cleared for `internal` and not for
  `confidential`.
- The security suite (`caelumora-kernel/tests/security_suite.rs`): an agent whose personality
  limits it to a role its owner does not hold reads, through the store, what an entry for its
  owner's role and one for its limit's role grant, and not what only one of them grants.

Documents: SECURITY.md (the context, `Admission`, the subject `Role`, the owner checking the
request, sub-agents), CONFIDENTIALITY.md (a limit as a ceiling, its roles not to be held),
STORAGE.md (the semi-join by the owner's roles, a limit judged by the store), and the doc comments
of `Personality::roles`, `Personality::limiting_roles`, `SecurityContext` and `covers_limit`. No
stored data changes.

## Done when

- What a personality's limit means is decided, implemented where it changes a question, and stated
  in SECURITY.md and CONFIDENTIALITY.md; `covers_limit` checked against it.

## History

- **2026-11-25** — Opened from the review of issue 597, step 2, where the comparison by the roles
  that count let a sub-agent be cleared higher than its starter.
- **2026-11-25** — Decided by the maintainer: a limit is a ceiling in every question, asked beside
  the owner's roles - in an access list as for a clearance (see *Decision*). Option 1 is rejected
  as it presumes an order among roles; option 2, as the issue described it, read the limit as
  taking the owner's place. The plan is put before the maintainer.
- **2026-11-25** — Plan accepted by the maintainer, the Postgres store's narrowing by the owner's
  roles alone included. Implemented: `Admission::limiting_roles`, asked beside `roles`
  (`admits_by_roles`); `SecurityContext::has_role` and `counting_roles` removed; `Views::subjects`
  by the owner's roles. Tests in `admission.rs`, `ownership.rs`, `confidentiality.rs`, the store
  suite (`lists_admit_what_check_decides`, a limit naming a role the owner does not hold, documents
  with two entries), the Postgres store and the security suite. SECURITY.md, CONFIDENTIALITY.md,
  STORAGE.md and the doc comments updated.
