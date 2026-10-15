# 369 — `security.rs` holds the actions, the attributes, the context, the access policy and its implementations in one file

**Status:** open
**Area:** Code structure

## The problem

`caelumora-model/src/security.rs` has 1331 lines and no description of the module. It holds, in
parts that each have a responsibility of their own:

| Lines | What |
|---|---|
| 20-329 | what is asked: `ActionType`, `Refused`, `Verb`, `CustomVerb`, `Requirement`, `JOIN`, `WRITE`, `Reclassified` |
| 331-471 | what is stored of who may: `SecurityAttributes`, `AclEntry`, `Subject`, `Grant`, `Memberships`, `NoMemberships`, `Secured` |
| 473-562 | in whose name something is done: `SecurityContext` |
| 564-795 | who decides: `AccessPolicy`, the questions on it (`AccessQuestions`), what enforces them (`Enforcement`), `AllowAllPolicy` |
| 797-976 | how the ownership policy decides: `OwnershipPolicy` |
| 978-1331 | 18 tests, most of them of `OwnershipPolicy` |

## Why it matters

Whoever reads how the ownership policy decides reads past every action there is; whoever adds an
action reads past the policy. Since [365](365-security-manager-is-an-access-policy.md) the file
holds a trait and its implementations under one name, without a description that says how the
parts fit together.

## Considerations

- **A module `security/`**, the paths unchanged through re-exports
  (`caelumora_model::security::…`, imported in 139 files of every crate):
  - `mod.rs`: a description of the module - who acts (`SecurityContext`), what is stored about who
    may (`SecurityAttributes`), what is asked (`ActionType`), who decides (`AccessPolicy`) -, and
    the re-exports;
  - `action.rs`: `ActionType`, `Refused`, `Verb`, `CustomVerb`, `Requirement`, `JOIN`, `WRITE`,
    `Reclassified`;
  - `attributes.rs`: `SecurityAttributes`, `AclEntry`, `Subject`, `Grant`, `Memberships`,
    `NoMemberships`, `Secured`;
  - `context.rs`: `SecurityContext`;
  - `access.rs`: `AccessPolicy`, `AccessQuestions`, `Enforcement`, `AllowAllPolicy`;
  - `ownership.rs`: `OwnershipPolicy`, with most of the tests.
  The tests go with what they test.
- **The name stays `security`, not `access`.** The module holds more than access:
  `SecurityContext` is also what the `ConfidentialityPolicy` clears (its roles and limiting roles),
  `SecurityAttributes` and `ActionType` are what any policy is asked with. Under `access`,
  `SecurityContext` would be misplaced or renamed too, and every import of the 139 files would
  change. `access` is already the name of the part that asks: `store::access` in the model, and
  the `access.rs` of the plugins, which hold their extensions of `AccessQuestions`; the submodule
  `security/access.rs` is that part of the core.
- **Not here:** `confidentiality.rs` (1022 lines), the other policy, stays as it is; whether it is
  split too is another issue.
- Moves only, no behaviour changes; `security.rs` through `git mv` to the file that keeps most of
  it, in a commit of its own, so that its history stays readable.

## Done when

- `security.rs` is a module `security/` as above, every path into it stays as it was, and its
  tests lie with what they test.
- No behaviour changes; clippy and the tests pass.

## History

- **2026-10-15** — Opened at the maintainer's request, under epic
  [349](349-core-review-epic.md), with the other moves of files
  ([366](366-runtime-rs-holds-four-types.md), [367](367-prompt-module.md),
  [368](368-component-module.md)), after the rename of
  [365](365-security-manager-is-an-access-policy.md).
