# 605 — A personality's limit means one thing for roles and another for clearances

**Status:** open
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

## Done when

- What a personality's limit means is decided, implemented where it changes a question, and stated
  in SECURITY.md and CONFIDENTIALITY.md; `covers_limit` checked against it.

## History

- **2026-11-25** — Opened from the review of issue 597, step 2, where the comparison by the roles
  that count let a sub-agent be cleared higher than its starter.
