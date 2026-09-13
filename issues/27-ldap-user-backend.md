# 27 — An LDAP user backend

**Status:** open
**Area:** Identity & access management

## The problem

Where users, groups and passwords are already managed centrally — Active
Directory, OpenLDAP, FreeIPA — no deployment will accept a second place to
create accounts. A file ([25](25-file-user-backend.md)) or a database
([26](26-database-user-backend.md)) would mean maintaining identities twice and
getting deprovisioning wrong, which is the one part of identity management that
must not be manual.

An LDAP-backed `UserProvider` authenticates against the directory and reads
group membership from it, so that the system holds no credentials of its own.

## Why it matters

It is the difference between a deployable system and a demo in most
organisations, and it is also the case that validates the trait design from
[25](25-file-user-backend.md): a directory verifies a password by *binding* as
the user, and never hands out anything comparable. Any provider interface that
assumes a stored hash can be fetched and compared cannot express LDAP at all.
Getting that wrong is cheap to fix now and expensive once two backends depend
on the wrong shape.

It also supplies, for free, the thing [23](23-acls-in-security-attributes.md)
needs and does not have: groups. A directory already knows them, and a
`GroupId` derived from a directory group is a far better starting point than a
group registry the system maintains itself.

## Considerations

### Bind is the authentication, not a hash comparison

The mechanism is: find the entry for the login name, then attempt a bind as
that DN with the supplied password. A successful bind is the authentication
result. This has consequences the other backends do not have:

- The search step usually needs its own service credential, which must come
  from the `SecretProvider` chain, never from `AppConfig` as a literal.
- The password passes through the process on every login, so the connection
  must be TLS (LDAPS or StartTLS), certificate validation must be on by
  default, and turning it off must be a visible, deliberate configuration
  option rather than an undocumented convenience.
- A failed bind and a missing entry must be indistinguishable to the caller,
  or the login form becomes a user enumeration oracle.

### Mapping a directory entry onto `User`

- `User` wants a username, an email, and — for now — `roles`. The attributes
  carrying those differ per directory (`uid` vs `sAMAccountName`,
  `mail`, `memberOf` vs `member` vs `posixGroup`), so the mapping has to be
  configuration: a base DN, a search filter, and an attribute map.
- The `UserId` problem from 25 and 26 appears again and is worst here. The DN
  changes when a user is moved between organisational units, and the login
  name changes on rename — neither is a stable key. Directories do have one
  (`objectGUID` in AD, `entryUUID` in OpenLDAP), and using it is the only
  mapping that survives an ordinary administrative action. That it is already a
  UUID in both cases makes the derivation trivial; that it is *not* what people
  will reach for first makes it worth writing down.

### Groups

`memberOf` is the easy case and is not universal: POSIX groups list their
members instead, nesting is expressed differently per directory, and resolving
nested groups can mean several queries per login. Since
[23](23-acls-in-security-attributes.md) will consult group membership during
authorization — which runs on every store operation and every tool call —
membership has to be resolved once at authentication and cached with the
session, not queried per check. The freshness bound is then a security
statement: how long after being removed from a group a user still acts with its
rights.

### Availability and the connection

- A directory outage means nobody can log in. Whether existing sessions
  survive is the same decision as in [26](26-database-user-backend.md) and
  should be answered the same way.
- Connections must be pooled and bounded, with timeouts on every operation:
  an unresponsive directory must fail the login rather than hold a request
  handler open.
- Referrals and multiple servers are normal in AD deployments; following
  referrals automatically can send credentials to a host that was not
  configured, which is a decision, not a default.

### What it must not try to be

No writes. No account creation, no password changes, no group management —
those are the directory's, and a backend that attempts them needs privileges
nobody will grant it. And no authorization: the directory supplies identity and
membership, the policy stays in [23](23-acls-in-security-attributes.md) and
[24](24-opa-security-manager.md).

### The adjacent option worth naming

For most modern directories the better integration is OIDC against the identity
provider in front of the directory, which removes the password from the
system's hands entirely and is a better fit for the REST server's token-based
path in [10](10-user-authentication.md). LDAP remains the answer where no such
provider exists, where the TUI must authenticate without a browser, or where
the directory is the only thing available — which is common enough that this
issue is worth doing on its own terms. But if both are built, OIDC is the
default and LDAP the fallback, not the other way round.

## Done when

- The `UserProvider` trait from [25](25-file-user-backend.md) has an LDAP
  implementation that verifies by bind and never requires a readable
  credential.
- Base DN, filter and attribute mapping are configuration; the service
  credential comes from the secret chain.
- TLS is on by default and the escape hatch is explicit.
- `UserId` derives from the directory's immutable entry identifier, so moves
  and renames do not create a new principal.
- Group membership is resolved at authentication, cached with a stated
  freshness bound, and usable as the group subject in
  [23](23-acls-in-security-attributes.md).

## History

- **2026-09-13** — Issue created as the third implementation of the shared
  user-provider seam, and as the case that constrains its design: bind-based
  verification rules out any interface built around comparing a stored hash.
