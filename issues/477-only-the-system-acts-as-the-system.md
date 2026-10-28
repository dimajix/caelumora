# 477 — Whoever acts in the system's name has the system's rights, though only the system itself should

**Status:** open
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

## Done when

- `SecurityContext::is_system()` exists, and `OwnershipPolicy` and the store's check of a mark ask
  it; no code compares a context's ids with `ActorId::System` by hand to mean "the system itself".
- `AgentManager::create` refuses a principal that is no user; `Agent::validate` refuses an agent
  owned by, or acting for, the system.
- The security suite shows that a context with the system as principal and an agent as caller has
  no special rights, and that an agent cannot be made in the system's name.
- SECURITY.md states the rule.

## History

- **2026-10-28** — Opened at the maintainer's request during issue 469, where the store's check of
  a deletion's mark asked whether the caller is the system while the policy asks the principal; the
  maintainer: only the system itself may act in the system's name.
