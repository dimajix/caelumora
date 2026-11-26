# 310 — Any secret can be named by whoever writes a component

**Status:** in progress
**Type:** feature
**Priority:** high
**Area:** Identity and access management
**Summary:** Any secret can be named by whoever writes a component: secrets have no owner and no access list, and the policy allows every `ReadSecret`.

## The problem

A component that talks to a remote system names its credential by a key, in its endpoint
(`RemoteEndpoint::auth`, for instance `auth.secret` of an `mcp_tool` or of an external agent). The
key is resolved through the `SecretManager` on every use, in the context of whoever acts - for an
agent its owner (issues [12](12-user-and-agent-permissions.md),
[255](255-whose-name-agents-act-in.md)). `SecretManager::resolve` asks the policy
`ActionType::ReadSecret(key)`, and the ownership policy always answers yes.

So nothing decides *which* secrets someone may use. Issue 12 settled *whose* a credential is - the
agent's owner's -, not whether that owner may name it. A secret carries no `SecurityAttributes`: it
is a key and a value, with nobody owning it and nobody granted it. Whoever may write a personality
or a preconfigured component could name any key the providers know - the operator's model API key,
another team's token - and point the endpoint at a URL of their choosing; the agent would send the
credential there on its first call.

The gap is latent today: personalities and components are written only by the seed and the
configuration, by whoever runs the server, and no REST route lets a user write them. The ownership
policy would already allow a user to create a personality of their own (`CreatePersonality`); the
gap opens as soon as anything lets them.

## Why it matters

- **Credentials leave the system.** Unlike content, which the flow rules keep inside, a secret is
  handed to a remote side by design; the only protection is who may name which secret where.
- **Users writing their own agents** is the obvious next step for personalities and preconfigured
  components, and it must not wait for this to be noticed then.
- **An external policy (issue [24](24-opa-security-manager.md))** cannot decide `ReadSecret` well
  either: it is given a key, not an object with an owner and an access list.

## Considerations

- **Secrets as entities with `SecurityAttributes`**: an owner and an access list, as every other
  entity has, so that "who may use this credential" is a grant (`Use`) like any other. Where they
  are kept - beside the providers' values, or as a registry of keys the configuration declares -
  is to be decided; the values stay with the providers.
- **When to ask**: when a component naming a secret is written (whoever writes it may use the
  secret), and when the secret is resolved (the principal may use it). The first refuses early and
  where the decision is made; the second holds if grants change afterwards. Probably both, by the
  pattern of issue 304: the act that names it asks, and the use asks as the operation it is.
- **What a secret may be sent to**: a grant could be bound to the endpoints it may be used with, so
  that a usable secret still cannot be pointed elsewhere.
- **The system's secrets**: those the configuration names for the server itself (a storage of
  workspaces, the model of the system) are the system's, used in its context.

## Decisions taken

Agreed with the maintainer on 2026-10-13; the options that were weighed are under *History*.

- **A secret of the configuration is the system's.** Nobody but whoever runs the server puts its
  values there, so it has no other owner. What decides is the access list: whom the system grants
  its use.
- **The attributes belong to a vault, not to the backend.** A vault is a named entry of the
  configuration: a backend (`SecretProvider`, unchanged - a file, the environment, a key vault),
  and beside it the access list, the origins its secrets may be sent to, and a fallback. The core
  reads these; a backend knows nothing of rights, so that a backend such as Azure Key Vault, which
  can carry neither an owner nor origins, needs nothing of its own. A vault without `acl` is the
  system's alone; without `origins`, its secrets may be sent anywhere (`*`).
- **A reference names its vault:** `vault/KEY` (`SecretRef`). Whether one may use it is decided
  from the configuration alone, before any backend is asked; nothing is hidden by another vault,
  and a refusal tells nothing of whether a key exists. A plain key is refused.
- **`user` is a reserved vault name:** the vault of the principal, for an agent its owner's (issue
  [343](343-users-bring-their-own-secrets.md)). The configuration cannot name a vault so. Until
  343, `user/…` is found nowhere.
- **A vault declares its own fallback** (`local: { fallback: env }`), one at most, chains allowed,
  cycles refused at startup. Not the other way round: no entry may change where the references to
  another vault lead without it being seen there. The fallback is followed on "not there", never on
  "not yours": a vault refused is refused, and the next one is asked only after the previous one
  was allowed and did not have the key. Rights and origins are those of the vault that answers.
  The default keeps today's convenience: `local` (`local/secrets.json`) falling back to `env`.
- **The question asked is `Use` of the kind `secret`**, with the vault's attributes - later, for a
  secret a user brings, with its own. `ActionType::ReadSecret(&str)` goes, and with it the policy's
  "always yes"; the verb `USE` moves from `caelumora-skills` into `caelumora-security`, one verb
  for skills and secrets. An external policy (issue 24) no longer sees the key, only the attributes.
- **Asked three times**, as issues 304 and 321 have it: when a component naming a secret is
  written, in the writer's name, before the verb; when it is built, in its owner's name; and at
  every use, in the name of whoever acts.
- **The registry learns which secret a component names from its kind** (`ComponentKind::reaches`):
  a kind that reaches a remote side declares it, the registry reads the `RemoteEndpoint` from
  `params.endpoint` itself. Not a free path, since no kind needs one; not a hook of the kind,
  which a plugin could make say less than it reads. `reaches` declares nothing else: since issue
  [516](516-the-runtime-trusts-the-agents-linked-into-it.md) no kind of a library is given the
  `SecretManager`, and every kind that reaches is a library's; its components resolve through the
  `Secrets` bound to the work.
- **The binding to origins is part of this issue.**

Agreed with the maintainer on 2026-11-26, as the plan was brought up to date with the code:

- **Writing and building ask through `Secrets`.** `Secrets::require_usable` asks what resolving
  asks, without a backend; `RemoteEndpoint::require_usable(&dyn Secrets)` asks it of its credential
  and its URL, as `bearer_token` resolves it. The registry holds the `SecretManager` and binds it to
  the name it asks in (`SecretManager::for_context`); the domain knows no `SecurityContext`.
- **An error of a backend is no "not there".** The fallback is followed only where the backend did
  not have the key; where it fails, the lookup fails, so that a vault that is down never lets
  another credential be sent in its stead.
- **A decision model's credential stays its asker's** (issue
  [551](551-a-decision-model-is-asked-past-the-model-plugin.md)): the operator grants the vault
  that holds it to whom may use the model, by its `acl`.
- **The default keeps the rule:** `local` and `env` without `acl` are the system's alone, so an
  agent names a secret of them only once the configuration grants it; `examples/config.yaml` does.
- **What the server sends nowhere** - the signing key of its tokens, the URL of a database, which
  is the secret itself - is resolved with no `sent_to`.

## Implementation plan

Brought up to date with the code on 2026-11-26 (see *History*). Four steps, each reviewed and
committed on its own, with its tests and documents.

### Step 1: references name their vault

No change of rights: `ReadSecret` is asked as before, of the reference.

- `SecretRef { vault, key }` (`caelumora-domain/src/secret.rs`): read from and shown as
  `vault/KEY`, a plain key refused; `SecretRef::USER_VAULT = "user"`.
- `VaultConfig` (`caelumora-domain/src/config.rs`): the backend (`BackendConfig`, flattened) and
  `fallback`. `SecretsConfig { providers }` becomes `SecretsConfig { vaults: BTreeMap<String,
  VaultConfig> }`, by default `local` (`file`, `local/secrets.json`, falling back to `env`) and
  `env`; made with `Backends::make_named` instead of `make_chain`. `AuthConfig::jwt_secret` becomes
  a `SecretRef`, by default `local/CAELUMORA_JWT_SECRET`.
- `Vault` (`caelumora-security/src/secret.rs`): a backend with what its configuration gives it.
  `SecretManager::new(vaults, policy)` fails on an unknown fallback, a cycle or the name `user`.
- `Secrets::resolve(&self, secret: &SecretRef)`, `SecretManager::resolve(context, secret)`: the
  vault's backend, its fallback on "not there", a backend's error the lookup's.
- `SecretError`: `NotFound` and `PermissionDenied` name the `SecretRef`; `UnknownVault` is new.
- Every reference becomes a `SecretRef`: `RemoteAuthConfig::Bearer { secret }`, the URL of
  `caelumora-store-postgres`, `jwt_secret`; `examples/config.yaml` and the personalities of
  `examples/seed/`.

### Step 2: `Use` of the kind `secret`

- `VaultConfig` gains `acl`; `Vault` carries `SecurityAttributes` (the system's, the configured
  list).
- `SecretRef::KIND`, the `EntityKind` `secret`; the verb `USE` moves from
  `caelumora-skills/src/access.rs` to `caelumora-security/src/action.rs`.
- `ActionType::ReadSecret` is removed; `SecretManager::resolve` asks
  `ActionType::custom(SecretRef::KIND, &USE, attributes)` of the vault that answers, its fallback
  only after it was allowed.
- `examples/config.yaml` grants its vaults.

### Step 3: origins

- `VaultConfig` gains `origins`, `*` if left out.
- `Secrets::resolve(&self, secret, sent_to: Option<&str>)`, `SecretManager::resolve(context,
  secret, sent_to)`: the origin is checked after `Use`, before the backend. `None` is what the
  server sends nowhere. `RemoteEndpoint::bearer_token` passes its URL.
- `SecretError::NotSentTo { secret, origin }`.

### Step 4: checked where a configuration is written and built

- `Secrets::require_usable(&self, secret, sent_to)`, `SecretManager::require_usable(context,
  secret, sent_to)`: what resolving asks, without a backend.
- `RemoteEndpoint::require_usable(&self, secrets: &dyn Secrets)`: of its credential and its URL.
- `ComponentKind::reaches()` (`caelumora-api/src/component.rs`): the kind reaches the
  `RemoteEndpoint` in `params.endpoint`. `mcp_server`, `a2a_agent`, `web_search` and the security
  suite's kinds that reach declare it.
- `ComponentRegistry::new` and `built_in` take the `SecretManager`. After `require_buildable`,
  `require_acceptable` (in the writer's name) and `build_with_defaults` (in the owner's) ask
  `require_usable` of every planned component whose kind reaches, `ref`s resolved. A missing or
  invalid `endpoint` is `InvalidConfig` there.
- `ComponentError::Secret(SecretError)`, so that the `Refused` stays the source the REST server
  finds (issue 323).

### Unchanged

- `SecretProvider`, and how a plugin registers a backend of secrets. Every access is a read.
- Who resolves in which name: the storages of workspaces, `caelumora-open-terminal`'s storage, the
  signing key and the database in the system's; a decision model for whoever asks; a component for
  whoever acts.

### Tests, documents, stored data

- The security suite: a user who cannot make an agent use a secret of a vault not granted to
  them, refused when writing, when building and when resolving; a secret not sent to an origin its
  vault does not allow; a fallback that is not followed on a refusal.
- SECURITY.md (*Secrets*), PERMISSION-CHECKS.md (rule 3, the tables), COMPONENTS.md (`reaches`,
  remote endpoints), CONFIGURATION.md (`secrets`, `auth`), TOOLS.md (credentials), WORKSPACE.md,
  `examples/config.yaml`, `examples/seed/`.
- Stored data: personalities and preconfigured components that name a secret without its vault
  are invalid and have to be deleted and seeded anew.

### Noticed beside it

The storages of workspaces read what is a rule of the core - `destination`, `shell` - in each
backend and hand it out through the provider's trait (`provider.destination()`); vaults keep such
rules out of the backends. To be aligned in an issue of its own, not here.

## Done when

- A secret has an owner and says who may use it; `ReadSecret` is decided by it and not always
  allowed.
- Naming a secret in a component that someone writes is refused unless they may use it.
- The security suite shows a user who cannot make an agent use a secret that is not theirs.
- A reference to a secret names its vault, and a secret is sent to no origin its vault does not
  allow.

## History

- **2026-10-11** — Opened while reviewing the permission concept (issue
  [304](304-review-of-the-permission-concept.md), F12): of the four actions the policy always
  allows, `ReadSecret` is the one whose "yes" hides a gap, since secrets have no owner to decide by.
  304 keeps `ReadSecret` as it is and documents that only the seed and the configuration name
  secrets until this is done.
- **2026-10-13** — Discussed with the maintainer and decided (see *Decisions taken*). Weighed and
  set aside: secrets as entities of the store, holding only their attributes - every secret a user
  may use would be declared twice, in its backend and in Caelumora, and a user as its owner would
  mean nothing while only the operator puts values there; attributes delivered by the backends -
  a key vault cannot carry them; one chain filtered by the access lists, the first vault that
  grants and has the key answering - convenient, but the same key could mean another credential
  for another principal, which an explicit vault in the reference rules out. Users bringing
  secrets of their own was split off into [343](343-users-bring-their-own-secrets.md).
- **2026-11-26** — Plan brought up to date with the code, and agreed with the maintainer. Since
  it was written, `caelumora-model` is gone: the references and the configuration are
  `caelumora-domain`'s, the `SecretManager` and the verbs `caelumora-security`'s (issue 517).
  Issue 516 added `Secrets`, bound to a name, which the plan did not know; it carries the reference,
  `sent_to` and `require_usable`, and `RemoteEndpoint::require_usable` takes it instead of a
  context. Since 516 no kind of a library is given the `SecretManager`, so `reaches` no longer
  implies it, and the check of a kind that uses it without reaching is moot.
  `ComponentContainer::with_defaults` is gone; the checks go into `require_acceptable` and
  `build_with_defaults`. The plan did not know the resolvers of `caelumora-store-postgres`,
  `caelumora-open-terminal` and `caelumora-model-systemone`. Decided besides (*Decisions taken*):
  a backend's error fails the lookup, a decision model's credential stays its asker's, the default
  vaults keep the rule, and what the server sends nowhere has no `sent_to`. The work is split into
  four steps.
- **2026-11-26** — Step 1 done: references name their vault (`SecretRef`, `vault/KEY`), the
  configuration names its vaults with their fallbacks, and the `SecretManager` reads a secret of
  its vault, asking the fallback only where the backend did not have the key. Deviating from the
  plan, confirmed by the maintainer: `SecretRef` is a value with rules in
  `value/secret_ref.rs`, as `Identifier`, its vault an `Identifier` and the vaults of the
  configuration named by one; each vault's backend is made with `make_one`, since `make_named`
  takes no `VaultConfig`; `SecretError::InvalidVaults` names what `SecretManager::new` refuses, and
  `UnknownVault` names the reference, not the vault alone.
- **2026-11-26** — Step 2: the ownership policy grants what a `Read` verb asks of what the system
  owns to everyone - seeded skills are used so -, so one verb for skills and secrets would have
  made a vault without `acl` everybody's, against *Decisions taken*. Decided with the maintainer:
  secrets get a verb of their own, `caelumora_security::secret::USE` (`UseSecret`), with a new
  requirement, `Requirement::Explicit(Grant::Read)` (first called `Granted`): granted by an entry
  of the access list or by owning, not by the system's ownership as such. Skills keep their `USE`
  in `caelumora-skills`;
  "one verb for skills and secrets" is withdrawn. Weighed and set aside: one verb, with a vault
  owned by nobody (`ActorId::None`), which the existing requirement would have decided alike - but
  a vault is the system's, and an external policy would see it owned by none; and the ownership
  policy no longer granting what the system owns for a kind's own verbs, which would have taken
  seeded skills from everybody.
- **2026-11-26** — Step 2 done: a vault carries the system's attributes with the access list its
  configuration gives it (`acl`), and `SecretManager::resolve` asks `UseSecret` of each vault of the
  way before it is read, a fallback only once the vault before did not have the key.
  `ActionType::ReadSecret` is gone, and with it the policy's "always yes". The test kernel has the
  vaults `test`, granted to everyone, and `withheld`, the system's alone; the agents' suite shows an
  agent refused a secret of a vault its owner is not granted, in her name, whoever asked. A secret
  is granted by the principal alone - an agent by its owner, not as the caller -, so that a context
  whose caller is the system grants nothing. `examples/config.yaml` keeps `local` and `env`, where
  the server's own secrets lie, the system's, and puts the agents' credentials into a vault of
  their own, `agents`, granted to every user; the seed names them there.
- **2026-11-26** — Decided with the maintainer: what `SecretManager::new` refuses - vaults that do
  not fit together - is a `ConfigError::Invalid`, the error of sections that do not fit together,
  not a `SecretError`, which says how reading fails; `SecretError::InvalidVaults` of step 1 is gone.
- **2026-11-26** — Step 3 done: a vault names where its secrets may be sent (`origins`, each an
  `Origin`, a value with rules: scheme, host and port, written as the URL standard writes an
  origin), anywhere if left out, nowhere but to the server itself if empty.
  `SecretManager::resolve` takes `sent_to`, `None` for what the server sends nowhere - the signing
  key, a database's URL, which is the secret itself; every vault of the way is checked, after
  `UseSecret` and before its backend is asked, and a refused origin is not passed over to a
  fallback (`SecretError::NotSentTo`). The review found the A2A agent sending its credential to
  the interface its card names, which the endpoint's origin did not stand for. Decided with the
  maintainer, deviating from the plan: `RemoteEndpoint::bearer_token(secrets, sent_to)` takes the
  URL of the request the credential goes with, and every component passes its own - the A2A
  agent its card's and its interface's; `Secrets::resolve(secret, sent_to: &str)` takes no `None`,
  since a component's credential always goes somewhere, which leaves no component a way past the
  origins; and the origins of every vault of the way count, not only of the one that answers, so
  that a vault that restricts them is never widened by its fallback - as `UseSecret` is asked of
  each (*Decisions taken*, "Rights and origins are those of the vault that answers", holds for
  rights so, and is replaced for origins). Not in the security suite: the origin is no question
  asked in someone's name; the security crate's tests hold it, and the A2A agent's own test of a
  card that names an interface elsewhere. The crates whose tests use the test vaults now ask for
  the feature `test-support` of `caelumora-security` themselves, which they had relied on the
  workspace for.
- **2026-11-26** — At the maintainer's request, the origins in the security suite after all: the
  test kernel has a third vault, `bound`, granted to everyone and sent to `https://bound.example`
  alone, and the agents' suite shows an agent's credential granted in its owner's name and refused
  for where its remote agent is reached.
