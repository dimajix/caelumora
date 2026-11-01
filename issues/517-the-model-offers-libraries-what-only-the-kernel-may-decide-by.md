# 517 — The model offers the libraries what only the kernel may decide by

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** A library links the model, and the model still holds the `SecurityContext`, the policies, the secret manager, the user providers and the traits of the stores, so the compiler lets a library name what only the kernel may decide by, and only a script finds it; the plan draws two crates out of the model, `caelumora-security` and `caelumora-store`.

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
  TranscriptStore}`), the checked store and the memory store, which name the context.

A library can name all of them and the compiler does not object; since 516 that is a rule kept by
`scripts/library-names.sh`, a grep over the libraries' code. It also leaves a seam in the API: as
the kernel may implement no foreign trait for a foreign type, `Params` is implemented for the
model's configurations of the kernel's backends (`FileSecretConfig`, `FileUserConfig`,
`AcceptAnyConfig`) in `caelumora-api`, which so names three things that are the kernel's.

## Why it matters

Not because a library could skip a check today. After 516 nothing a library is given takes a
context or hands out what decides: the API names of these modules only `Refused`, `StoreError`,
`StoreResult`, `Filter`, `HistoryField` and `Secrets`; a component is given only the services its
kind declares it uses (`ComponentRegistry::dispatch`, `services.only`), and a kind of a library
may declare none of the kernel's (`keeps_libraries_to_their_side`). A context a library builds
reaches nothing but what it built itself.

What matters is the cut, and that it holds by the compiler, in both directions:

- **Towards the libraries.** What decides is in a crate a library cannot link, checked by
  `library-deps.sh` as the kernel is - not by a grep for names that a renamed import slips past.
- **Towards the kernel.** The model seals today what the kernel must not reach: the checked store
  (`CheckedStore`), the access it asks (`Access`) and how an admission narrows a list are
  `pub(crate)` in the model, so that the kernel opens entities only through `EntityStores::open`,
  checked. Moved into the kernel, every module of it could reach them; in crates of their own, the
  seal stays.
- **The model in one sentence.** It holds the data and the rules over its values, and nothing that
  decides in someone's name.

## Considerations

- **(a) Into the kernel.** The context, the policies, the managers and the traits of the stores
  move into `caelumora-kernel`. Recommended in 516 (decision 7), but it gives up the seal of the
  stores above: in the kernel, `CheckedStore` and `Access` are visible to all of it.
- **(b) Crates of their own** beside the model, which the kernel links and the API does not. Two,
  as what they hold depends one on the other: `caelumora-security`, the context and what decides by
  it, and `caelumora-store`, the stores, which name the context. Same boundary towards the
  libraries as (a), and the seals the model has today stay.
- **(c) Nothing moves.** `library-names.sh` is deleted, and ARCHITECTURE.md says why the boundary
  holds without it. Cheap, but the model keeps holding what the kernel decides by, and the seam in
  the API stays.
- **Cost of (a) and (b).** About 12,000 lines leave the model, tests included -
  `store/suite.rs` alone has 2,300 -, and every `use` of them changes: the kernel, the plugins, the
  stores, the REST server, the tests. No behaviour changes; the libraries' tests keep building the
  kernel's world as a dev-dependency.

## Decision

(b), taken on 2026-11-01: two crates, `caelumora-security` and `caelumora-store`. What has to be
the kernel's because of how the crates depend on each other - the user providers, the backends of
the secrets and the configuration - moves into the kernel.

## The plan

### The layers

```
caelumora-model  <-  caelumora-security  <-  caelumora-store  <-  caelumora-kernel
       ^                                                                 |
caelumora-api  <---------------------------------------------------------+
```

The stores name the context and the policies (`EntityStore::get(ctx, …)`, `EntityStores::open(…,
policies)`), so `store` stands on `security`. The API stays on the model alone; a library links
neither of the two crates.

### What goes where

| Crate | holds |
|---|---|
| `caelumora-model` | the entities and values; `SecurityAttributes`, `AclEntry`, `Subject`, `Grant`, `Secured`, `SecuredMut`; `Refused`; the values of confidentiality (`Confidentiality`, `Compartment(s)`, `Classification`, `LabelCauses`, `Clearance(s)`, `Place`, `Reading`, `Visibility`); `PrivacyConfig` and `SessionPrivacy`; `Secrets`, `SecretError`; the words of the stores (`Stored`, `EntityKind`, `Revision`, `Journaled`, `Updatable`, `Enclosing`, `Address`, `NoEntity`, `StoreError`, `query::*`, `HistoryField`); `parse_yaml` |
| `caelumora-security` | `SecurityContext`; `ActionType`, `Verb`, `CustomVerb`, `Requirement`; `AccessPolicy`, `AccessQuestions`, `Enforcement`, `OwnershipPolicy`, `AllowAllPolicy`, `Policies`, `Admission`, `Memberships`; `ConfidentialityPolicy`, `ConfidentialityQuestions`, `ConfidentialityRules`, `may_reach`; `PrivacyPolicy` and its policies; `SecretManager`, `SecretsFor`, the trait `SecretProvider` |
| `caelumora-store` | `EntityStore`, `EntityStores`, `StateStore`, `HistoryStore`, `TranscriptStore`, `Persistence`, `PersistenceBackend`, `Presence`, `Deleted`, `documents` (`CheckedStore`, `DocumentStorage`), `access`, `kept_chain`, `StoreMemberships`, `MemoryStore`, `suite` |
| `caelumora-kernel`, in addition | the user providers and `UserManager`, the backends of the secrets (`EnvSecretProvider`, `FileSecretProvider`), `AppConfig` with its sections, and the three `impl Params` |

Three things cannot go into `caelumora-security`:

- **The user providers** read the records of the users (`EntityStore<User>`), so in `security`
  they would make it depend on `store`, which depends on it.
- **The configurations of the backends** (`FileSecretConfig`, `FileUserConfig`,
  `AcceptAnyConfig`) implement `Params` of the API. By the orphan rule the impl lies with the trait
  or with the type, and `security` may not link the API. With the backends in the kernel - where
  its `Backends` makes them, as it makes the storages - the impls lie there too.
- **`AppConfig`** names the policies, the backends and the stores; it is what the kernel is built
  from.

### Seams the cut shows

- **Contexts built in the model.** `Agent::security_context` and `Approver::security_context`
  become constructors of `SecurityContext`; `Questions::resolve` takes the owner and the initiator
  it reads of the context, not the context.
- **Privacy.** `PrivacyConfig::policy` and `SessionPrivacy::permits` are methods of model types
  that ask the policy; with `PrivacyPolicy` in `security`, the choice of the policy by its
  configuration goes with it (`<dyn PrivacyPolicy>::of(config)`), and its only caller,
  `ConfidentialityRules`, asks it there.
- **What becomes visible.** Items `pub(crate)` today but used across the new boundaries become
  `pub`, each looked at on its own: `Admission::may_admit` (asked by the memory store),
  `Cursor::after` and `Cursor::id` (by the stores' pages); test helpers of the model used by the
  stores' tests (`meets_exactly_its_equals`, `store::testing`) go behind `test-support`.
- **Two `Memberships`.** `security::Memberships`, the trait of who is in which group, and
  `store::history::Memberships`, who was in a channel when; in different crates they no longer
  clash, but one of them may want a name of its own.

### The steps

Each step builds on its own and is a commit of its own; none changes behaviour.

1. **`caelumora-store`.** The stores leave the model; they name `caelumora_model::security` until
   step 3. The model keeps the words of the stores. `caelumora-store-json` and
   `caelumora-store-postgres` link it for `PersistenceBackend`, and the kernel for the plugin.
2. **Into the kernel.** The user providers, `UserManager`, the backends of the secrets and
   `AppConfig` move into the kernel, and the three `impl Params` leave the API for the kernel's
   `backend.rs`.
3. **`caelumora-security`.** The context, the policies, the policy of confidentiality and privacy,
   and the secret manager leave the model; the store and the kernel name them there. The model
   builds no context any more. `library-deps.sh` refuses `caelumora-security` and
   `caelumora-store` among a library's normal dependencies, as it refuses the kernel, and
   `library-names.sh` is deleted. The documents follow: ARCHITECTURE.md (the crates, the
   principle), CRATES.md (its graph is written by `crate-graph.sh`), SECURITY.md, STORAGE.md,
   AGENTS.md (what each crate holds).

The order is forced: the model cannot depend on `security` while it holds stores that name the
context, so the stores leave first.

## Done when

- No library can name the context, a policy, the secret manager, a user provider or a store trait:
  `cargo tree -e normal` of a library finds neither `caelumora-security` nor `caelumora-store`, and
  `library-deps.sh` checks it.
- The model builds no `SecurityContext` and names nothing of `caelumora-security` or
  `caelumora-store`.
- `caelumora-api` names nothing of the kernel's backends.
- `scripts/library-names.sh` is deleted.

## History

- **2026-11-01** — Opened from decision 22 of issue
  [516](516-the-runtime-trusts-the-agents-linked-into-it.md): the policies, the context and the
  store traits stay in the model there, checked by a script, and leave it here.
- **2026-11-01** — Planned: crates of their own (b) rather than the kernel (a), as they keep the
  seal the model has today towards the kernel as well; `caelumora-security` and `caelumora-store`,
  the user providers, the backends of the secrets and `AppConfig` into the kernel, `PrivacyPolicy`
  into `security` by its subject. *Why it matters* corrected: since 516 a library can skip no check;
  the gain is the cut, held by the compiler in both directions.
