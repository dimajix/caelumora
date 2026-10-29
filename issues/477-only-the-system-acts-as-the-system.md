# 477 — Whoever acts in the system's name has the system's rights, though only the system itself should

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** Identity and access management
**Summary:** `OwnershipPolicy` allows everything to a context whose principal is the system, whoever its caller is, and `AgentManager::create` lets the system own an agent, which then acts with all the system's rights.

## The problem

`OwnershipPolicy::check` answers `true` to every question as soon as `principal_id` is
`ActorId::System` (`caelumora-model/src/security/ownership.rs:170`), and `admission` narrows
nothing for it (`:219`). Who the caller is, is not asked.

A context whose caller is not its principal is built by `SecurityContext::next` alone, and only
for agents: an agent acts in the name of its principal, which is its owner
(`Agent::security_context`, `caelumora-runtime/src/actor/mod.rs:190`, issue
[341](341-identity-ownership-delegation.md)). An agent owned by the system would act with the
system's rights - read, change, delete and purge everything.

`AgentManager::create` makes the agent's owner `context.principal_id` and does not ask whether that
is a user (`caelumora-runtime/src/agent/manager.rs:89`): an agent made in the system's name is the
system's. Nothing in the program does so today - seeds make no agents, a template makes them in its
user's name -, so the door is open, not used. The managers of sessions, workspaces, groups,
projects and skills all refuse a principal that is no user (`OwnerNotAUser`); the agents' does not.

"The caller is the system" is written by hand in two places that mean different ids: the policy
asks the principal, the store's check of a deletion's mark (`Access::by_caller`, issue
[469](469-a-failed-restore-renews-the-mark.md)) asks the caller.

## Why it matters

The system's context is meant for the runtime acting on its own, for work that follows from an act
already permitted (`SecurityContext::system`). Lending its rights to anything that runs on behalf of
someone else - an agent, which a language model drives - turns one careless creation into an actor
with every right, unchecked by any policy.

## Considerations

- **A. Only the system itself acts in the system's name.** A named predicate,
  `SecurityContext::is_system()` - caller and principal both the system -, which the policy and
  the store ask instead of comparing ids by hand. And no agent acts for the system:
  `AgentManager::create` refuses a principal that is no user (`OwnerNotAUser`), as the other
  managers do, and `Agent::validate` refuses an agent owned by the system, so that no write past
  the manager makes one either. Then no context has the system as principal and another caller.
- **B. Only the predicate.** The policy asks `is_system()`; a context with the system as principal
  and an agent as caller gets no blanket permission, but is judged as any principal is - which
  still admits it to everything the system owns. Leaves the agent owned by the system possible.
- **C. Clamp in `SecurityContext::next`.** A continued context with another caller never keeps the
  system as principal. Hides the error where the agent is made, instead of refusing it.

Recommendation: A - the rule stated by the maintainer, named once, and the one way of breaking it
closed where agents are made.

## Decision

A, with three changes:

1.  **`is_system()` is caller, owner and principal the system**, not caller and principal alone:
    the clearance (`ConfidentialityRules::cleared_for`) asks the owner, a third place comparing by
    hand, and one predicate serves all three. The system acting on someone's behalf - caller and
    owner the system, principal a person - is allowed, with that person's rights.
2.  **Only the system is refused as an agent's owner**, not every owner that is no user: an agent
    that starts a subagent is to own it. `Agent::validate` refuses it; `AgentManager::create` asks
    nothing of its own, since only the system itself has the system as its principal now, and a
    typed error would repeat the store's check for what only the program's own code can attempt.
3.  **An agent of the system goes as a concept**: its calls were approved by whose channel it is
    (issue [14](14-approval-privilege.md)). Nobody answers for an agent of nobody, which everybody
    may read, until issue [491](491-every-entity-has-an-owner.md) gives every entity an owner; the
    channel `ApproveToolCall` carried for that case goes.

## Done when

- `SecurityContext::is_system()` exists, and `OwnershipPolicy`, the store's check of a mark and
  the clearance ask it; no code compares a context's ids with `ActorId::System` by hand to mean
  "the system itself".
- `Agent::validate` refuses an agent owned by, or acting for, the system.
- Tests show that a context with the system as principal and an agent as caller has no special
  rights, and the security suite that an agent cannot be made in the system's name.
- SECURITY.md states the rule.

## History

- **2026-10-28** — Opened at the maintainer's request during issue 469, where the store's check of
  a deletion's mark asked whether the caller is the system while the policy asks the principal; the
  maintainer: only the system itself may act in the system's name.
- **2026-10-29** — Decided: A, with `is_system()` asking the owner as well, the system refused as an
  agent's owner rather than every owner that is no user, and an agent of the system gone as a
  concept; issue 491 opened for entities without an owner.
- **2026-10-29** — Done on branch `issue-477-only-the-system`: `SecurityContext::is_system()`, asked
  by `OwnershipPolicy`, `Access::by_caller` and `cleared_for`; `Agent::validate` refuses an agent of
  the system; `answers_for` lets nobody approve for an agent of nobody, and `ApproveToolCall` and
  `AccessQuestions::may_approve` no longer carry the channel. SECURITY.md states the rule; the
  agent of the system is gone from APPROVALS.md, PERMISSION-CHECKS.md and SKILLS.md, and
  CONFIDENTIALITY.md and STORAGE.md speak of the system itself.
