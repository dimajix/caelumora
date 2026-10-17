# 310 — Any secret can be named by whoever writes a component

**Status:** open
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
  "always yes"; the verb `USE` moves from `caelumora-skills` into `caelumora-model`, one verb for
  skills and secrets. An external policy (issue 24) no longer sees the key, only the attributes.
- **Asked three times**, as issues 304 and 321 have it: when a component naming a secret is
  written, in the writer's name, before the verb; when it is built, in its owner's name; and at
  every use, in the name of whoever acts.
- **The registry learns which secret a component names from its kind** (`ComponentKind::reaches`):
  a kind that reaches a remote side declares it, the registry reads the `RemoteEndpoint` from
  `params.endpoint` itself, and only such a kind is given the `SecretManager`. Not a free path,
  since no kind needs one; not a hook of the kind, which a plugin could make say less than it
  reads.
- **The binding to origins is part of this issue.**

## Implementation plan

### New types

- `SecretRef { vault, key }` (`caelumora-model/src/secret/mod.rs`): read from and shown as
  `vault/KEY`; `SecretRef::USER_VAULT = "user"`.
- `VaultConfig` (`caelumora-model/src/config.rs`): the backend (`BackendConfig`, flattened) and
  `acl`, `origins`, `fallback`.
- `Vault` (`caelumora-model/src/secret/mod.rs`, private to the module): the provider, its
  `SecurityAttributes` (owner the system, the configured access list), its origins
  (`may_be_sent_to(url)`) and the name of its fallback.
- The `EntityKind` `secret`, and the custom verb `USE` moved into `caelumora-model/src/security.rs`.

### Changed interfaces

- `SecretProvider`: unchanged.
- `SecretsConfig { providers: Vec<BackendConfig> }` becomes `SecretsConfig { vaults:
  BTreeMap<String, VaultConfig> }`, made with `Backends::make_named` instead of `make_chain`.
  `AuthConfig::jwt_secret` becomes a `SecretRef`, by default `local/CAELUMORA_JWT_SECRET`.
- `SecretManager::new` takes the named vaults and fails on an unknown fallback, a cycle or the
  name `user`.
- `SecretManager::resolve(context, key: &str)` becomes `resolve(context, secret: &SecretRef,
  sent_to: Option<&str>)`: find the vault, ask `Use` of its attributes, check the origin, only then
  ask its backend; on "not there", the same for its fallback. `None` is the server's own use, sent
  nowhere - the signing key of its tokens.
- New `SecretManager::require_usable(context, secret, sent_to)`: the checks without asking a
  backend, for writing and building.
- `SecretError` gains `UnknownVault` and a variant for an origin the vault does not allow.
- `ActionType::ReadSecret` is removed; `ActionType::custom(SECRET, &USE, attributes)` instead.
- `RemoteAuthConfig::Bearer { secret: SecretRef }`; `RemoteEndpoint::bearer_token` passes its URL
  as `sent_to`; new `RemoteEndpoint::require_usable(secrets, context)`.
- `ComponentKind::reaches()`: declares that the kind reaches the `RemoteEndpoint` in
  `params.endpoint`, and implies `uses::<SecretManager>()`. `mcp_tool`, `a2a_agent` and the
  security suite's `remote_at_work` declare it instead of `uses::<SecretManager>()`; a kind that
  declares `uses::<SecretManager>()` without it is refused when the runtime is built.
- `ComponentRegistry::new` and `built_in` take the `SecretManager` (made before the registry).
  After `require_asked`, both `require_acceptable` and `ComponentContainer::with_defaults` ask
  `require_usable` of every planned component whose kind reaches - `ref`s resolved, so that
  whoever takes a shared component into their agent must be allowed its secret themselves. A
  missing or invalid `endpoint` is `InvalidConfig` there.
- `ComponentError::Secret(SecretError)`, so that the `Refused` stays the source the REST server
  finds (issue 323).

### Unchanged

- How a plugin registers a backend of secrets.
- The storages of workspaces and the signing key resolve in the system's name, now with a vault in
  their reference.

### Tests, documents, stored data

- The security suite: a user who cannot make an agent use a secret of a vault not granted to
  them, refused when writing, when building and when resolving; a secret not sent to an origin its
  vault does not allow; a fallback that is not followed on a refusal.
- SECURITY.md (*Secrets*), PERMISSION-CHECKS.md (rule 3, the tables), COMPONENTS.md (`reaches`),
  CONFIGURATION.md (`secrets`, `auth`), `examples/config.yaml`, `examples/seed.yaml`.
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
