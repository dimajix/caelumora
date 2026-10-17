# 25 — A text-file user backend

**Status:** in progress
**Type:** feature
**Priority:** high
**Area:** Identity and access management
**Summary:** There is no source of identities to authenticate against; a hand-edited user file is the smallest one, and defines the provider seam.

## The problem

[10](10-user-authentication.md) needs a source of identities to authenticate against, and there is
none. What exists today is bootstrapping code: the REST server takes a `--user` argument,
`setup_initial_data` looks the name up and creates the user with a fresh `Uuid::new_v4()` if it is
missing, and the TUI does the same with a default id. Nothing verifies a credential, because there
is no credential anywhere in the model — `User` has a username, an email and `roles`, and that is
all.

The smallest useful backend is a text file: a list of users with their credentials, edited by hand,
read by the runtime. It is what makes a single-machine or small-team deployment authenticate at all
without requiring a database or an identity provider.

## Why it matters

Without it, 10 can only be finished by writing users into the entity store, where credential
material does not belong and where they have to be created through an API that itself has no
authentication. A file breaks that circle: it is populated out-of-band, before the process starts,
by someone who already has access to the machine.

It is also the backend that keeps the local paths honest. The TUI and `caelumora-local-client` need
*some* identity; a file gives them a real one instead of a fabricated `SecurityContext`, so there is
one code path rather than a bypass that has to be grepped for.

## Considerations

### This defines a trait, not just an implementation

A file is one source of users; a database ([26](26-database-user-backend.md)) and a directory
([27](27-ldap-user-backend.md)) are others, and all three should plug into the same seam. The
precedent is `SecretProvider` / `SecretManager`: a small trait, several implementations, an ordered
chain in `AppConfig`, first match wins. A `UserProvider` in `caelumora-model` alongside it would be
the consistent shape, and this issue — as the first of the three — is where that trait should be
designed.

Two details decide whether the trait survives contact with the other two backends:

- **Lookup and credential verification must be separate operations, and verification must not assume
  the credential can be read back.** A file backend holds a password hash and compares; an LDAP
  backend verifies by binding and can never hand out anything to compare against. A trait shaped as
  "fetch the user, then check its hash" cannot express the second and would have to be rewritten
  for 27.
- **The credential kind is not fixed.** Password today; a bearer token or an API key are the obvious
  next ones, and they may be verified by a different provider than the one that owns the identity.

### What the file contains, and in what format

- `FileSecretProvider` is the precedent worth following in behaviour: a plain file, re-read on every
  lookup so a user can be added or disabled without restarting, a missing file treated as "no
  entries" rather than a hard error.
- The format should carry structure, because `User` already has more than a name: id, username,
  email, and the roles or group names that [23](23-acls-in-security-attributes.md) resolves into
  effective roles. That rules out an `htpasswd`-style `user:hash` line format, which is the most
  familiar shape for this kind of file but has room for exactly two fields. YAML matches
  `AppConfig`, JSON matches the secrets file and `JsonFileStore`; picking one of those two is a coin
  flip that should be settled by which one people will hand-edit.
- **Credentials are hashes, never passwords.** Argon2id (or bcrypt/scrypt) with a per-entry salt,
  verified in constant time, and the file itself checked for restrictive permissions on load with a
  loud warning if it is world-readable. The alternative worth weighing is that the file holds only
  identities and names a *secret key* per user, resolved through the existing `SecretProvider` chain
  — it reuses a mechanism that already exists and keeps all credential material in one place, at the
  cost of two files to keep in sync for what is meant to be the simplest possible backend.

### Stable ids are the part that will bite

`UserId` is a UUID that ends up in `SecurityAttributes.owner_id`, in `Metadata.owner`, in every
persisted session and every agent. If the file does not carry ids, they are regenerated per process
— which is exactly what `setup_initial_data` does today — and every ownership reference in the store
dangles after a restart. Either the file carries an explicit id per user, or ids are derived
deterministically from the username (a UUIDv5 over a fixed namespace). The derived form is
friendlier to hand-editing and makes renaming a user a breaking operation; the explicit form is the
opposite. Whichever is chosen, it is not a file-backend decision — 26 and 27 have the identical
problem mapping an external key onto `UserId`, so it belongs in the trait.

### Relationship to the entity store

`EntityStore` already has `create_user` / `get_user` / `find_user_by_name`, and `JsonFileStore`
already persists users as one JSON file each under `runtime/users/`. That is a *store* —
machine-written, id-addressed, read-write — and it is not the same thing as a hand-edited authority
file, so having both is not automatically duplication. But two places that can answer "who is alice"
is a split brain unless one of them is declared authoritative:

- The provider is authoritative and users are provisioned into the entity store on first successful
  authentication, so the rest of the system keeps reading `User` from the store as it does now. Then
  `update_user` has to be rejected or reconciled for file-backed users — a read-only backend means
  some fields are not the store's to change.
- Or `EntityStore`'s user methods delegate to the provider chain, and the store stops owning users
  entirely. Cleaner conceptually, but it turns a store call into a network call for 26 and 27.

### Failure behaviour differs from the secret chain

`FileSecretProvider` logs a corrupt file and falls through to the next provider, which is right for
a secret — a missing secret is a missing feature. For authentication it is not: a typo in the user
file that makes it unparseable must not silently hand the question to the next provider or, worse,
look like "user does not exist". A malformed users file should fail loudly at load, and an
unreadable one should deny rather than fall through.

## Done when

- A `UserProvider` trait exists, with lookup and credential verification as separate operations and
  no assumption that a stored credential can be read back, plus a chain configured in `AppConfig`
  next to `secrets`.
- A file-backed implementation exists: format decided, re-read on change, hashed credentials,
  permission check, loud failure on a malformed file.
- `UserId` assignment is stable across restarts, by an explicit id in the file or a documented
  derivation, and the same rule is available to 26 and 27.
- Who owns the `User` record — the provider or the `EntityStore` — is decided and written down,
  including what `update_user` does for a read-only backend.
- The REST server's `--user` flag means "authenticate as", and the TUI and local client obtain an
  identity the same way instead of fabricating one.

## History

- **2026-09-13** — Issue created as the first concrete backend for [10](10-user-authentication.md);
  it carries the design of the shared `UserProvider` trait that [26](26-database-user-backend.md)
  and [27](27-ldap-user-backend.md) build on.
- **2026-09-21** — Split and sequenced together with [10](10-user-authentication.md). Order:
  1. The `UserProvider` trait and its chain, from this issue, without the file backend.
  2. An `AcceptAnyUserProvider` as an explicit development stand-in: accepts any username and
     password, must be enabled in the configuration, is never the default, and warns loudly at
     startup.
  3. [10](10-user-authentication.md): login page, JWT cookie, middleware, all written against the
     trait only.
  4. The file backend of this issue, as a drop-in behind the trait.

  Reason: the file backend carries decisions of its own (format, explicit or derived ids, who owns
  the `User` record, what `update_user` may do) that 10 does not depend on, while a trait designed
  without a real consumer tends to be cut wrong. 10 is that consumer.

  **Decided: how `UserId`s are assigned is left to each provider.** The trait's contract is only
  that a provider returns the same id for the same user for as long as it knows that user, including
  across restarts. Random ids are fine if the provider persists them; derived ids (e.g. UUIDv5 over
  the username) are fine too. This replaces the "same rule for 26 and 27" expectation in *Done
  when*: there is a shared contract, not a shared rule.
- **2026-09-21** — Step 1 done: `UserProvider`, `Credential`, `AuthError` and the `UserManager`
  chain in `caelumora-model/src/user_provider.rs`; `AcceptAnyUserProvider` in
  `user_provider_impl/accept_any.rs`; a `users` section in `AppConfig` (`create_user_manager`).
  Decisions:
  - `Credential` is borrowed and `#[non_exhaustive]`, and its `Debug` impl redacts the secret. Only
    `Password` exists so far.
  - In the chain, `Ok(None)` falls through; `InvalidCredentials` and `Backend` errors end it — for
    lookups as well as for authentication. Unknown users are reported as `InvalidCredentials`.
  - Lookups are not gated through the `SecurityManager`, unlike `SecretManager::resolve`:
    authentication is what produces the context such a check would need.
  - No default chain: without a `users` section nobody can authenticate.
  - `AcceptAnyUserProvider` keeps users in the `EntityStore` with random ids, so it finds the user
    `setup_initial_data` bootstraps from `--user`, and its ids are as stable as the configured
    store. Its find-or-create is not atomic; accepted rather than giving `caelumora-model` a runtime
    dependency on tokio.
  - Not decided yet, still open here: who owns the `User` record in general (the accept-any provider
    simply uses the store), and the file backend. Nothing uses the `UserManager` yet; that starts
    with [10](10-user-authentication.md).
- **2026-10-16** — The empty crates `caelumora-client-api` and `caelumora-local-client` were
  removed: of the local paths, only the TUI is left to obtain its identity through the providers.
  Without it, its user has no roles, and since clearances are checked it reaches no channel above
  `public` - with `examples/config.yaml` it does not start ("… is not cleared for channel …").
- **2026-10-16** — The TUI obtains its identity through the providers (`--user <name>`,
  `Credential::Trusted`), so it starts with `examples/config.yaml` again, its user cleared as
  `accept_any` configures them. A provider that verifies a secret refuses `Trusted` for a user it
  knows; with a file backend, the TUI would need a password - to be decided with it.
