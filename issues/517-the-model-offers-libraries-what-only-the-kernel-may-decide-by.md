# 517 — The model offers the libraries what only the kernel may decide by

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** A library links the model, and the model still holds the `SecurityContext`, the policies, the secret manager, the user providers and the traits of the stores, so the compiler lets a library name what only the kernel may decide by, and only a script finds it.

## The problem

Issue [516](516-the-runtime-trusts-the-agents-linked-into-it.md) cut the libraries of the agent side
from the kernel: a library links `caelumora-api`, the model and other libraries, never
`caelumora-kernel`, which `scripts/library-deps.sh` checks from the pre-commit hook. What is the
kernel's - its managers, controllers, views and runtimes - the compiler now refuses a library. But
the model still holds much of what decides:

- the context and the policies: `security::{SecurityContext, ActionType, AccessPolicy,
  AccessQuestions, Enforcement, OwnershipPolicy, Policies, AllowAllPolicy}`,
  `confidentiality::{ConfidentialityPolicy, ConfidentialityQuestions, ConfidentialityRules}`,
  `privacy::PrivacyPolicy`;
- the secret manager and its providers (`secret::{SecretManager, SecretProvider, env, file}`), the
  user providers (`user_provider::*`, `UserManager`) and the configuration (`config::AppConfig`);
- the traits of the stores (`store::{EntityStore, EntityStores, StateStore, HistoryStore,
  TranscriptStore}`) and the memory store, which name the context.

A library can name all of them and the compiler does not object. Since 516 that is a rule kept by
`scripts/library-names.sh`, a grep over the libraries' code. It also leaves a seam in the API: as
the kernel may implement no foreign trait for a foreign type, `Params` is implemented for the
model's configurations of the kernel's backends (`FileSecretConfig`, `FileUserConfig`,
`AcceptAnyConfig`) in `caelumora-api`, which so names three things that are the kernel's.

## Why it matters

The trust boundary of 516 holds by the compiler only as far as the dependency graph reaches. A
library that builds a `SecurityContext` of its own, or asks a policy itself, is caught by a script
that matches names - a rule that a renamed import or a new name of the model slips past. The
intent of 516 - that what decides cannot be skipped - is fully kept only when the library cannot
name it at all.

## Considerations

- **(a) Into the kernel.** The context, the policies, the secret manager, the user providers, the
  configuration and the traits of the stores move from the model into `caelumora-kernel`; the
  model keeps the entities and their values. What the API names of them stays where the API can
  reach it: the errors - `StoreError`, `Refused` - which the API's errors carry, and `Secrets`,
  which `ActorRuntime::secrets` answers. The store implementations of the model (memory, JSON)
  move with their traits, or into a crate of their own. Recommended in 516 (decision 7).
- **(b) A crate of their own** beside the model - `caelumora-policy`, say - which the kernel links
  and the API does not. Smaller moves, one more crate; the boundary is the same.
- **Cost.** Over 7,800 lines leave the model, and every `use` of them changes - the kernel, the
  plugins, the stores, the tests. No behaviour changes; the libraries' tests keep building the
  kernel's world as a dev-dependency.
- **What follows.** `scripts/library-names.sh` has nothing left to check and is deleted, or keeps
  what the model still has to offer; the three `impl Params` return to the kernel's `backend.rs`
  with the configurations they are for.

## Done when

- No library can name the context, a policy, the secret manager, a user provider or a store trait:
  `cargo tree -e normal` of a library finds no crate that holds them.
- `caelumora-api` names nothing of the kernel's backends.
- `scripts/library-names.sh` is deleted, or checks only what the model still offers by intent.

## History

- **2026-11-01** — Opened from decision 22 of issue
  [516](516-the-runtime-trusts-the-agents-linked-into-it.md): the policies, the context and the
  store traits stay in the model there, checked by a script, and leave it here.
