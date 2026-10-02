# 142 — Roles are strings nothing gives a meaning to

**Status:** in progress
**Area:** Identity and access management

## The problem

A `User` carries `roles: Vec<String>`, filled by its user provider (`accept_any` gives everybody
`"user"`), but nothing reads them: no policy decides by a role, and nothing says which roles exist
or what they stand for. Step 5 of [133](133-classification-model.md) needs them: a user's
clearance - how far they may see into which compartments - is not a property of the person to be
listed one by one, but follows from what they are in the organisation, which is what roles are.

## Why it matters

- Clearances ([133](133-classification-model.md), step 5) are to be derived from roles; without
  roles, the only alternative is a list of users in the configuration, which does not scale and
  duplicates the directory.
- Access lists ([23](23-acls-in-security-attributes.md)), an administrator
  ([125](125-invisible-admin.md)) and the approval privilege ([14](14-approval-privilege.md)) all
  want to grant something to a role rather than to each user.
- The directories users come from - a user file ([25](25-file-user-backend.md)), a database
  ([26](26-database-user-backend.md)), LDAP ([27](27-ldap-user-backend.md)) - have groups; what a
  group of the directory becomes in Caelumora needs one place.

## Considerations

- **Where roles come from.** The user providers deliver them, as they do today - from the user file,
  from LDAP groups. Whether a provider maps its groups to roles itself, or delivers them as they are
  and the configuration maps them, is open.
- **Which roles exist.** Configured by the administrator, like the compartments: a role unknown to
  the configuration means nothing, and is logged rather than silently ignored.
- **Roles to clearances.** A clearance is a level for what is general and a level per compartment
  ([133](133-classification-model.md), step 5: "Alice may see internal of finance and confidential
  of HR"). The configuration maps each role to a clearance; a user's clearance is the union of their
  roles' - per compartment the highest level any of them grants. A user without a role that grants
  anything gets a default.
- **An agent's clearance** is its owner's: it acts in its owner's name and must not see more than
  the person responsible for it.

## Done when

- Which roles exist and what each grants is configured in one place, the user providers deliver
  roles, and a user's clearance is derived from them.

## History

- **2026-09-30** — Opened while deciding step 5 of [133](133-classification-model.md): the
  maintainer placed clearances on the users, derived from their roles by rules - and roles have no
  meaning yet.
- **2026-09-30** — Decided with the maintainer: which roles exist and what each grants is configured
  in `AppConfig`. How roles reach a user is the user provider's rule: for now its configuration
  gives named users a list of roles; mapping directory groups (LDAP, [27](27-ldap-user-backend.md))
  comes later. A user whose roles grant no clearance sees only what is `public`.
- **2026-09-30** — With step 5 of [133](133-classification-model.md): the `roles` section of
  `AppConfig` names the roles and the clearances each grants; `accept_any` gives roles to named
  users and `default_roles` to everybody else, updated at every login; unknown roles and
  compartments are configuration errors. What remains: roles from the real user providers - a user
  file, a database, LDAP groups - and roles for anything but clearances.
