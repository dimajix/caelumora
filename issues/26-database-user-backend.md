# 26 — A database user backend

**Status:** open
**Area:** Identity & access management

## The problem

A text file ([25](25-file-user-backend.md)) is enough for one machine and a
handful of people. It is not enough as soon as users are created and disabled
while the system runs, as soon as more than one runtime process must see the
same answer, or as soon as the users already exist in a database that someone
else operates. That is the case a database-backed `UserProvider` covers.

There is nothing in the workspace today: users live in whichever `EntityStore`
is configured, which is `MemoryStore` by default and `JsonFileStore` at best,
and neither holds a credential.

## Why it matters

This is the backend that makes a real deployment possible, and it is the one
that has to coexist with an existing landscape. In most organisations the user
table is not ours to design — it belongs to an application that was there
first — and a backend that can only work against a schema we invented is a
backend that will not be used.

It also shares its infrastructure with three other open issues. Postgres is
already the intended target for the entity store
([03](03-postgres-entity-store.md)), the state store
([04](04-postgres-state-store.md)) and the event recorder
([05](05-postgres-event-recorder.md)). Whatever those settle on for
connections, pooling and migrations must be settled once, not four times.

## Considerations

### Our schema or theirs

This is the decision the rest depends on, and the two answers are different
products:

- **Ours.** We define the table, ship migrations, and the system can create
  users, store hashes, reset passwords and disable accounts. It is the natural
  continuation of 03 — at which point "database user backend" is barely a
  separate backend at all, it is the Postgres `EntityStore` plus credential
  columns, and the argument for a separate `UserProvider` implementation
  becomes weak.
- **Theirs.** An existing table, read-only, mapped onto `User` by a
  configurable query. Nothing is created, nothing is reset, and the schema can
  change under us. This genuinely needs its own provider, and it needs the
  column mapping to be configuration rather than code.

They are not mutually exclusive — read-only against a foreign schema and
read-write against our own could be two configurations of one provider — but
pretending a single implementation covers both without saying so is how the
write path ends up silently broken against a foreign database.

### Credentials

- Hashing is the provider's business only in the "ours" case: Argon2id,
  per-entry salt, constant-time verification, and a pepper resolved through the
  `SecretProvider` chain rather than sitting in the config file. In the
  "theirs" case the hash format is whatever that system chose, and supporting
  it means supporting *its* scheme — bcrypt, PBKDF2, something home-grown —
  which should be an explicit, bounded list rather than an open promise.
- The database connection string is itself a credential and must be a secret
  reference, exactly as `mcp_tool`'s `auth.secret` already is, never a literal
  in `AppConfig`.
- Failed-attempt throttling and lockout belong here rather than in the REST
  layer, because they are per-identity and need somewhere durable to count.
  They also introduce a denial-of-service shaped footgun: lockout by username
  lets anyone lock anyone out.

### Mapping an external key onto `UserId`

`UserId` is a UUID and it is embedded in every persisted ownership reference. A
foreign table's key is an integer or a string, so the mapping has to be stable
across restarts and across processes — a UUIDv5 over the external id is the
obvious construction. This is the same problem 25 has with hand-edited files
and 27 has with directory entries, which is why it belongs in the shared trait
and not in three implementations.

Renaming is where it gets sharp: if the derivation uses the login name, a
renamed user becomes a different principal and loses everything they own. If it
uses an immutable internal key, the mapping survives renames — which is an
argument for insisting on an immutable key column even when the login name
looks unique.

### Groups are an input to roles, not a subject

[23](23-acls-in-security-attributes.md) does not accept groups as ACL subjects:
entries name an actor or a role, and group names delivered by a user backend
reach the ACL only through a role assignment stored in the system. This backend
therefore supplies group names — whatever the schema calls them — and does not
care what they mean; the mapping onto roles is not its configuration to hold,
precisely so that the same mapping applies no matter which backend a deployment
runs.

### It is on the hot path

Once 23 exists, a subject's effective roles are read during authorization, and
authorization runs on every store operation and every tool call. A database
round trip per check is not viable, so resolution happens once at
authentication and is cached with the session — along with the decision of how
quickly a disabled account, or a revoked group membership, actually stops
working, which is that cache's lifetime stated as a security property.

### Availability

If the database is unreachable, nobody can authenticate. That is correct and
unavoidable. What has to be decided is whether an *already authenticated*
session keeps working — a token issued before the outage, validated against
cached data — or whether everything stops. Both are defensible; silently
getting one while believing the other is not.

### Not a policy store

The temptation with a database is to put the authorization rules in it too.
They belong in [23](23-acls-in-security-attributes.md) as attributes on the
entities, or in a policy engine ([24](24-opa-security-manager.md)). This
backend answers who someone is and which groups the schema places them in, and
stops there — including the mapping from those groups onto roles would put
privilege assignment inside a backend's configuration.

## Done when

- The `UserProvider` trait from [25](25-file-user-backend.md) has a
  database-backed implementation, with the read-only-foreign-schema and
  owned-schema cases explicitly distinguished in configuration.
- Connection details come from the secret chain, never from plaintext config.
- The external-key-to-`UserId` mapping is stable and survives a rename.
- Group lookup feeds the role resolution of
  [23](23-acls-in-security-attributes.md) and is cached with a stated
  freshness bound.
- Schema ownership, migrations and pooling reuse whatever
  [03](03-postgres-entity-store.md) establishes rather than introducing a
  second mechanism.

## History

- **2026-09-13** — Issue created alongside [25](25-file-user-backend.md) and
  [27](27-ldap-user-backend.md) as the second implementation of the shared
  user-provider seam.
