# 491 — An entity can be stored without an owner, and is then taken for the system's

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Identity and access management
**Summary:** `SecurityAttributes` defaults to the owner `ActorId::None`, which every seeded component bundle, personality and template without attributes gets, and which `unowned()` and the policies treat as the system's - though everything is to have an owner.

## The problem

`SecurityAttributes` derives `Default`, and `ActorId::default()` is `ActorId::None`. Component
bundles, personalities and session templates read their attributes with `#[serde(default)]`, so
whatever a seed file (`examples/seed.yaml`) writes without attributes is stored with the owner
`None`. Nothing refuses it: no store asks whether an entity has an owner.

What such an entity is, is said only in passing: "An entity without an owner (`ActorId::None`, as
in the demo data of a seed file) belongs to the system"
(`caelumora-model/src/security/attributes.rs:19`), and `SecurityAttributes::unowned()` treats
`None` and `System` alike - everybody reads it (`Admission`,
`caelumora-model/src/security/admission.rs:71`), and the calls of an agent of nobody are approved
by whose channel it is (`OwnershipPolicy::answers_for`). Tests make agents and
sessions of nobody throughout (`SecurityAttributes::default()`, `Agent::new(…, ActorId::None)`).

`ActorId::None` means something else everywhere else: no actor - the context nobody filled in,
which has no rights (issue [11](11-agent-ownership.md)), a message whose sender is not known, an
entry of the history no actor made. As an owner it stands for the system by a convention.

## Why it matters

- Everything is to have an owner, who answers for it. An owner that is nobody answers for nothing,
  and what is the system's is the system's by a default nobody chose, not by a decision.
- Every check that asks for the system's things has to ask for nobody's as well (`unowned()`), and
  each one that does not is a gap - issue [477](477-only-the-system-acts-as-the-system.md) found one
  in approving the calls of an agent of nobody, which is left as a special case until this issue.

## Considerations

- **A. The store refuses an entity without an owner, and a seed names the system.** Whatever is
  created or changed in any store must have an owner other than `None`, checked once where the
  store checks what it writes, not in each kind's `validate`. `SecurityAttributes` has no
  `Default`; what a seed leaves out is the system's, set where a seed is read. `unowned()` becomes
  "the system's", and `answers_for` loses its case for an agent of nobody. The tests give their
  entities an owner. Stored data with the owner `None` - every store seeded so far - is refused
  and has to be seeded anew.
- **B. An owner of its own for "nobody"** - an `Anonymous`. Renames `None` without changing what
  it is: an owner who answers for nothing.
- **C. Leave it**, the convention documented.

Recommendation: A.

## Decision

A, with two choices:

1.  **A seed's entities are the system's by a serde default** - `#[serde(default =
    "SecurityAttributes::systems")]` on the attributes of the kinds a seed holds: component
    bundles, personalities, session templates and skills -, not by an owner every entry of a seed
    file would have to write. The skills' seeding no longer sets the owner itself.
2.  **The owner is named positively**: a user, an agent - which may own a subagent - or the
    system. `SecurityAttributes::validate` refuses nobody and an approver alike, neither of whom
    answers for anything.

## Done when

- No store keeps an entity whose owner is `ActorId::None`.
- What a seed leaves without attributes is owned by the system.
- `SecurityAttributes::unowned()` no longer exists as "the system or nobody"; nothing treats
  `ActorId::None` as an owner.

## History

- **2026-10-29** — Opened at the maintainer's request while planning issue 477: things are always
  to have an owner, and `None` as an owner breaks that.
- **2026-10-29** — Issue 477 took the channel owner's approval for an agent of nobody away: nobody
  answers for it now, the one special case left in `answers_for` for this issue to remove.
- **2026-10-29** — Decided: A, a seed's entities the system's by a serde default, the owner named
  positively; on branch `issue-491-every-entity-has-an-owner`.
- **2026-10-29** — Done: `SecurityAttributes` has no `Default`; `systems()` gives what a seed
  leaves out, and `validate` refuses an owner that is no user, agent or system, so that no store
  keeps one. `unowned()` is gone, `Admission::unowned` is `systems`, Postgres's `UNOWNED` is
  `SYSTEM`, and `answers_for` has no case left for an agent of nobody. The tests give their
  entities the system or a user as owner, the store suite shows that an entity of nobody is not
  stored, and the app's seeding test that what the example seed holds is the system's.
  SECURITY.md states the rule.
