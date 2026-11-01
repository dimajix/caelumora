# 517 — The model offers the libraries what only the kernel may decide by

**Status:** done
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
  user providers (`user_provider::*`, `UserManager`) and the choice of the policies by the
  configuration (`AppConfig::create_access_policy`, `create_confidentiality_policy`);
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
the kernel's because of how the crates depend on each other - the user providers and the backends
of the secrets - moves into the kernel. The configuration (`AppConfig`) stays in the model, as data;
what it chooses the policies by becomes a constructor of `Policies` (amended the same day, see
History).

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
| `caelumora-model` | the entities and values; `SecurityAttributes`, `AclEntry`, `Subject`, `Grant`, `Secured`, `SecuredMut`; `Refused`; the values of confidentiality (`Confidentiality`, `Compartment(s)`, `Classification`, `LabelCauses`, `Clearance(s)`, `Place`, `Reading`, `Visibility`); `PrivacyConfig` and `SessionPrivacy`; `Secrets`, `SecretError`; the words of the stores (`Stored`, `EntityKind`, `Revision`, `Journaled`, `Updatable`, `Enclosing`, `Address`, `NoEntity`, `StoreError`, `query::*`, `HistoryField`); the configuration (`AppConfig`, `parse_yaml`) |
| `caelumora-security` | `SecurityContext`; `ActionType`, `Verb`, `CustomVerb`, `Requirement`; `AccessPolicy`, `AccessQuestions`, `Enforcement`, `OwnershipPolicy`, `AllowAllPolicy`, `Policies` with the policies the configuration chooses (`Policies::configured`), `Admission`, `Memberships`; `ConfidentialityPolicy`, `ConfidentialityQuestions`, `ConfidentialityRules`, `may_reach`; `PrivacyPolicy` and its policies; `SecretManager`, `SecretsFor`, the trait `SecretProvider` |
| `caelumora-store` | `EntityStore`, `EntityStores`, `StateStore`, `HistoryStore`, `TranscriptStore`, `Persistence`, `PersistenceBackend`, `Presence`, `Deleted`, `documents` (`CheckedStore`, `DocumentStorage`), `access`, `kept_chain`, `StoreMemberships`, `MemoryStore`, `suite` |
| `caelumora-kernel`, in addition | the user providers and `UserManager`, the backends of the secrets (`EnvSecretProvider`, `FileSecretProvider`), and the three `impl Params` |

Two things cannot go into `caelumora-security`:

- **The user providers** read the records of the users (`EntityStore<User>`), so in `security`
  they would make it depend on `store`, which depends on it.
- **The configurations of the backends** (`FileSecretConfig`, `FileUserConfig`,
  `AcceptAnyConfig`) implement `Params` of the API. By the orphan rule the impl lies with the trait
  or with the type, and `security` may not link the API. With the backends in the kernel - where
  its `Backends` makes them, as it makes the storages - the impls lie there too.

The configuration stays in the model: `AppConfig` is data, and names a backend by its type and
parameters (`BackendConfig`), not by its configuration. Only two of its methods name policies,
`create_access_policy` and `create_confidentiality_policy`; they become `Policies::configured`,
which the `KernelBuilder`, the seed and `caelumora-app` ask. In the kernel, `AppConfig` would make
`caelumora-logging`, which reads its `LoggingConfig` and links the model alone, link the kernel.

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

1. **Into the kernel.** The user providers and `UserManager` (`caelumora_kernel::user_provider`)
   and the backends of the secrets (`caelumora_kernel::secret`) move into the kernel, and the three
   `impl Params` leave the API for the configurations they are for.
2. **`caelumora-store`.** The stores leave the model; they name `caelumora_model::security` until
   step 3. The model keeps the words of the stores. `caelumora-store-json` and
   `caelumora-store-postgres` link it for `PersistenceBackend`, and the kernel for the plugin.
3. **`caelumora-security`.** The context, the policies, the policy of confidentiality and privacy,
   and the secret manager leave the model; the store and the kernel name them there, and
   `AppConfig`'s two methods that choose the policies become `Policies::configured`. The model
   builds no context any more. `library-deps.sh` refuses `caelumora-security` and
   `caelumora-store` among a library's normal dependencies, as it refuses the kernel, and
   `library-names.sh` is deleted. The documents follow: ARCHITECTURE.md (the crates, the
   principle), CRATES.md (its graph is written by `crate-graph.sh`), SECURITY.md, STORAGE.md,
   AGENTS.md (what each crate holds).

The order is forced: the user providers read the users through `EntityStore<User>`, so they leave
the model before the stores; and the model cannot depend on `security` while it holds stores that
name the context, so the stores leave before it.

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
- **2026-11-01** — Plan amended before step 1: the user providers read the users through the
  stores, so they leave the model first, and step 1 and 2 change places; `AppConfig` stays in the
  model, as data, and its two methods that choose the policies become `Policies::configured` in
  step 3, so that `caelumora-logging` keeps linking the model alone.
- **2026-11-01** — Step 1: the user providers, `UserManager` and the backends of the secrets
  (`EnvSecretProvider`, `FileSecretProvider`) moved into the kernel, as
  `caelumora_kernel::user_provider` and `caelumora_kernel::secret`; the three `impl Params` lie
  beside the configurations they are for, and `caelumora-api` names nothing of the kernel's
  backends any more. After the review of the step, they follow the kernel's other backends: each
  module registers its own (`secret::register_backends`, `user_provider::register_backends`, as
  `content` and `terminal` do), and their configurations and `FileUserProvider` are private to it.
  `User::new` is public, as its maker is no longer in the model. `argon2` and `tokio` are no longer
  among the model's dependencies, but for its tests. No behaviour changes. Stored data: none.
- **2026-11-01** — Step 2: `caelumora-store`. The stores left the model - `EntityStore`,
  `EntityStores`, `Persistence`, `PersistenceBackend`, `StateStore`, `Presence`, `Deleted`, the
  checked store and its access, the history and transcript stores, `kept_chain`,
  `StoreMemberships`, `MemoryStore`, the suite and `ContendedStore`; the model keeps the words of
  the stores (`caelumora_model::store`: `Stored`, `EntityKind`, `Revision`, `Journaled`,
  `StoreError`, `query`, `HistoryField`). What a test stores its world by went with them
  (`caelumora_store::testing::{stored_agent, record_users}`), as did the one test of the model that
  opened a store (a user's profile); the policies a test opens the stores by stay with `Policies`
  (`permitting_all`, `by_default_rules`, under `test-support`). Made public as the stores ask them
  across the new boundary: `Admission::may_admit`, `Cursor::after` and `Cursor::id`, `meeting`.
  The kernel, the REST server and the stores' plugins link the crate, the crates on both levels
  with `kernel`, `caelumora-agents` with `test-support`, the other libraries for their tests only.
  No behaviour changes. Stored data: none.
- **2026-11-01** — Step 3: `caelumora-security`. The context, the actions and their verbs, the
  access policy with `Admission` and `Memberships`, the policies of confidentiality and of privacy,
  `Policies` and the secret manager with its providers' trait left the model, which keeps
  `SecurityAttributes`, `Refused`, the values of confidentiality and of privacy, and `Secrets`. The
  model builds no context any more: `Agent::security_context` and `Approver::security_context` are
  `SecurityContext::of_agent` and `of_approver`, and `Questions::resolve` takes the owner and the
  initiator. `AppConfig`'s two methods that chose the policies are `Policies::configured`, and
  beside it `ConfidentialityRules::configured`, which `caelumora-app` and a test of the seed ask
  for the rules alone - a constructor the plan did not name. A session's policy of privacy is
  chosen as `<dyn PrivacyPolicy>::of`. The policies a test opens the stores by are
  `caelumora_security::testing`'s, the classifications and the actors tests write
  `caelumora_model::confidentiality::testing`'s and `caelumora_model::security::testing`'s. The
  verb `USE` of a skill moved to its access, under `kernel`, so that the data of `caelumora-skills`
  links no policy. `library-deps.sh` refuses `caelumora-security` and `caelumora-store` among a
  library's normal dependencies, and `library-names.sh` is deleted.
  After the review of step 2: `KeptItem` is the model's again, beside the fields that address its
  shape (`HistoryField`); the stores' dependencies are narrowed (`uuid` without `v4`, `chrono` for
  the suite alone), and the crates whose tests use nothing of the stores' `test-support` link them
  without it; module docs and documents that still placed the stores in the model are corrected.
  Every point of *Done when* holds. No behaviour changes. Stored data: none.
