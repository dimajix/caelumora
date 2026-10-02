# 23 — ACLs in `SecurityAttributes`

**Status:** open
**Area:** Identity & access management

## The problem

`SecurityAttributes` holds exactly one field, `owner_id: ActorId`, and nothing reads it
([11](11-agent-ownership.md)). There is no way to express that a second user may read a session,
that a role may start sessions from a template, or that one agent — and only that one — may execute
a given tool. Access is therefore all-or-nothing per deployment, and with `DummySecurityManager` the
answer is always "all".

What is missing is the *data*: an access control list on the objects being protected, saying which
subject may perform which kind of operation. The decision itself stays where it is —
`SecurityManager` remains the single policy decision point; ACLs are only the attributes it reads.

Three things have to be decided together, and the third is the one with no obvious answer:

1.  **Which entities carry `SecurityAttributes` at all.**
2.  **What a subject is** — an `ActorId`, or a role, of which there is today only an unread
    `User.roles: Vec<String>`.
3.  **What an entry grants.** `Create`, `Read`, `List`/`Find`, `Write`, `Delete`, `Execute` is the
    obvious verb set, but not every verb means anything for every entity kind, and a verb set that
    is two thirds inapplicable is a sign that the model is wrong rather than merely incomplete.

## Why it matters

Every issue in this area is blocked on the absence of this data. [10](10-user-authentication.md)
produces an authenticated identity with nothing to check it against.
[12](12-user-and-agent-permissions.md) asks how to combine two sets of rights that do not exist.
[14](14-approval-privilege.md) needs to say who is entitled to approve. [22](22-skill-management.md)
needs to say who may load which skill. Each of them can be answered once there is a place to write
the answer down, and none of them before.

It also matters that this lands *before* there is production data. Retrofitting an ACL onto stored
entities means choosing a default for every existing object, and every default is wrong for
somebody.

## Considerations

### Which entities need `SecurityAttributes`

Today the field exists on `Agent`, `Session` and `Channel`, seeded from `SessionContext::owner` when
a template is instantiated. The proposal adds skills and tools. The full inventory is larger:

- **`Session`, `Channel`, `Agent`** — already have the field, nothing reads it.
- **`Personality`** — defines what an agent *is*; whoever may edit a personality controls every
  agent instantiated from it. Needs both an edit right and an instantiate right, and those are not
  the same right.
- **`SessionTemplate`** — same argument: starting a session from a template is the interesting
  operation, editing it is the dangerous one.
- **`ComponentDescriptor`** — preconfigured components are referenceable from anywhere via `kind:
  "ref"`, and an `mcp_tool` descriptor names a secret. A component descriptor is the most privileged
  object in the store and the one with no attributes at all today.
- **`User`** — who may read, list and update a user.
- **`Role`** — does not exist as an entity yet (below), and needs attributes of its own.
- **`RoleAssignment`** — the mapping from actors and external groups onto roles is itself the most
  privilege-granting object in the system: whoever may write `ldap-group ops -> role admin` has
  granted admin. It needs attributes for the same reason a role does, and it must not be reachable
  by anyone who can merely edit a backend's configuration file.
- **`Skill`** ([22](22-skill-management.md)) — the loading right is the whole point of that issue.
- **Tools** are the awkward case, because a tool is not a stored entity. Tools are produced by
  `ToolProvider` components at build time, and an `mcp_tool`'s list is discovered from a remote
  server at runtime — so no ACL written in advance can enumerate them. Either the ACL lives on the
  `ComponentDescriptor` and applies to everything that component exposes (coarse, but it matches
  where the credential already sits), or tool entries are name patterns (`github-mcp__*`), which is
  the same namespacing convention `mcp_tool` already uses. Per-tool entries for tools that do not
  exist until connect time cannot be validated at authoring time either way.
- **Secrets** are worse: `ActionType::ReadSecret(&str)` names a key, and a key is a string in a
  provider, not an object. Either secrets get a descriptor entity that can carry attributes, or the
  ACL is part of the provider configuration. This should be decided here rather than left to
  [12](12-user-and-agent-permissions.md), which assumes the answer.
- **State** (`AgentState`, `SessionState`, `ChannelState`) deliberately gets nothing of its own:
  `StateStore` does not even take a `SecurityContext` today, and state belongs to exactly one
  entity. It should inherit, not carry a second copy that can drift.
- **Messages and history** likewise. Access to messages follows channel membership; a per-message
  ACL would be a weaker duplicate of it.

### Ownership is already stored twice

`Metadata` carries `owner: UserId` *and* `scope: Scope`; `SecurityAttributes` carries `owner_id:
ActorId`. They can disagree, the types differ (an agent can own something, a `Metadata.owner`
cannot), and `Channel` has `SecurityAttributes` but no `Metadata` at all while `Personality` has
`Metadata` but no `SecurityAttributes`. Adding an ACL next to a duplicated owner field makes three
sources of truth. The overlap has to be resolved first — most likely by `Metadata` keeping name,
description and scope, and everything security-relevant moving into `SecurityAttributes`.

### What a subject is: actors and roles, never groups

Three subject vocabularies are already half-started in the code and none is finished:

- `ActorId` — real, used everywhere, covers users and agents.
- `Scope::GroupScope { group_id: Uuid }` — a group id in the scope hierarchy, with no `Group`
  entity, no `GroupId` newtype and no membership anywhere.
- `User.roles: Vec<String>` — exists on every user and is read by nothing.

An ACL entry names an `ActorId` or a **`RoleId`**, and nothing else. Groups are deliberately not a
subject type, even though every directory has them, because a group answers a different question
than an ACL asks. A group is *who* — externally defined, with whatever meaning the organisation gave
it (`team-platform`, `berlin-office`). A role is *what, in this system* — defined by us, and
therefore something an ACL can be validated against when it is written.

The reason this matters here rather than being a terminology preference: we do not control the
directory. If entries referenced directory groups, every ACL would be coupled to a foreign naming
scheme that we cannot check at authoring time, that changes without us, and that does not exist at
all in a deployment running the file backend ([25](25-file-user-backend.md)). The three user
backends would each supply a different subject vocabulary. With roles as the only collective
subject, all ACLs speak one language, and the deployment's reality arrives at exactly one place
instead of being spread across every object's ACL.

That one place is the **role assignment**: a mapping from an actor, or from an external group name
delivered by a user backend, onto a role. A user's effective roles are the union of the roles
assigned to them directly and the roles their groups map to, resolved once at authentication and
cached with the session — which is exactly the resolution [26](26-database-user-backend.md) and
[27](27-ldap-user-backend.md) already have to perform for membership.

Two properties keep this from becoming a second permission mechanism:

- **A role is a subject label, not a bundle of rights.** It names a set of principals and holds no
  permissions of its own; rights come only from ACL entries, on objects and on scopes. This has to
  be written down explicitly, because the word suggests the opposite — in textbook RBAC a role *is*
  a permission set. If a role could carry rights directly there would again be two places to look
  when answering why somebody may do something, which is the thing this whole issue exists to avoid.
- **An assignment carries a `Scope`**, defaulting to `GlobalScope`. That is the difference between
  "alice is an admin" and "alice is an admin *of this session*", it costs one field, and `Scope`
  already exists with a working `contains()`. Without it, session- or channel-local roles get
  re-invented later as a parallel mechanism.

Consequences for what is already in the code:

- `User.roles: Vec<String>` stops being dead and becomes the direct half of the assignment — though
  it should hold role identifiers rather than free strings, and for a directory-backed user the
  roles are computed from the mapping rather than stored.
- `Scope::GroupScope { group_id: Uuid }` should become a role scope or be removed. Leaving a group
  id in the scope hierarchy while groups are not a subject type anywhere else is precisely the kind
  of third, unfinished vocabulary this section is trying to end.
- Allow-only union stays trivial: the effective right set is the union over the subject's direct
  entries and all of its roles, with no precedence rule to define.

What is given up is the ability to write a directory group straight into an ACL. In an organisation
with hundreds of directory groups that is mapping work — but mechanical work, done once, and ACLs
are rarely written against hundreds of groups. The coupling avoided is worth more.

### Identifiers

Both subject kinds are identified by a UUID: `ActorId` as it already is, and `RoleId` as a new
`define_id!` newtype. A role's name lives in its `Metadata` alongside its description, and nothing
references it.

For `ActorId` this only confirms what the user backends already require. A `UserId` ends up in
`SecurityAttributes.owner_id`, in `Metadata.owner` and in every persisted session, and it has to
survive a rename in a directory we do not control — which is why [25](25-file-user-backend.md),
[26](26-database-user-backend.md) and [27](27-ldap-user-backend.md) all derive it from an immutable
external key rather than from a login name. `ActorId::Approver(Uuid)` has no name to use in the
first place.

For `RoleId` the reason is renaming, and renaming is not the rare event it looks like: a deployment
introduces a uniform naming scheme after the fact and renames everything it has, or somebody finds a
typo in a role that is already referenced from a hundred ACLs. Neither of those is a change of
meaning, and neither should require touching a single reference. A name used as a key makes both of
them a migration; a UUID makes them an edit of one field.

The cost is that the readable part is gone exactly where things are read: hand-written ACLs, a role
assignment file, a Rego policy in [24](24-opa-security-manager.md). That is a real loss and it has
to be paid back deliberately rather than assumed away:

- Anything hand-authored refers to roles by name and resolves them to ids when it is loaded, failing
  loudly on a name that does not exist. This is possible precisely because roles are ours and
  enumerable — the property that disqualified directory groups as subjects.
- Anything rendered to a human — an error, a log line, an ACL listing — resolves the id back to the
  name. A permission error naming only a UUID is the failure mode [02](02-better-error-messages.md)
  describes.
- Policy input documents carry both, and policies match on the id. A Rego rule matching on a name
  would reintroduce exactly the breakage the UUID prevents, so the name in the input is for reading
  and decision logs only.

### What an entry grants — and why the verb list is the hard part

`Create` / `Read` / `List`+`Find` / `Write` / `Delete` / `Execute` applied uniformly to every kind
is the simplest thing that could work, and its problem is visible immediately: `Execute` on a
`Session` means nothing, `Create` on an existing `Agent` means nothing, and an ACL editor would
offer both.

Options:

- **A verb enum per entity kind.** Type-safe and self-documenting, but `SecurityAttributes` is one
  struct shared by all kinds, so it would need a type parameter or a per-kind variant, and every
  store, serializer and API gains a case per kind. The safety is real; the cost is spread over the
  whole codebase.
- **A single generic verb set, with nonsensical combinations simply never granted.** Cheap and
  uniform, and wrong entries are inert rather than dangerous — but nothing stops them being written,
  and "inert" is only true as long as no future verb reuses the name.
- **Derive the verbs from `ActionType`.** This is the option worth taking seriously, because the
  enumeration already exists and is already the authoritative list of securable operations:
  `GetPersonality`, `PutSessionTemplate`, `CallTool`, `ReadSecret` and the rest. Instead of
  inventing a second vocabulary that has to be kept in sync by hand, add one total function
  `ActionType -> (EntityKind, Verb)`: a single exhaustive `match`, so the compiler forces every new
  `ActionType` variant to declare what it is. ACL entries stay generic, and `Verb::applies_to(kind)`
  — derived from the image of that mapping — lets the authoring path reject `Execute` on a session
  instead of storing it.

Independently of which is chosen, two of the proposed verbs are not instance permissions at all:

- **`Create` has no instance to hang on.** The object does not exist when the check runs, so the
  permission is really "may create an object of this kind *here*" — a property of the container, not
  of the created object.
- **`List` / `Find` is elementwise `Read`.** If listing were a separate grant, `list_personalities`
  would have two answers that can disagree. The defensible semantics is that a list returns exactly
  what the caller may read, which makes `List` a filter rather than a verb — at the cost of N checks
  per call, and that cost is the reason the alternative keeps getting proposed. Once there is a
  Postgres store ([03](03-postgres-entity-store.md)) the filter wants to be a query predicate rather
  than a post-filter, which argues for keeping the ACL shape simple enough to translate into SQL.

So the model that falls out is: **instance permissions** (`Read`, `Write`, `Delete`,
`Execute`/`Use`) on the object, and **collection permissions** (`Create`, and enumeration if it
stays separate) on a *scope*. `Scope` already exists, already forms a hierarchy and already has
`contains()` — it is the natural home for the second kind, and it gives inheritance for free.

### Inheritance, and allow-only

If a `Channel` inherits its `Session`'s entries, a session template does not have to restate an ACL
per channel, and containment already has a meaning via `Scope::contains`. The price is that a child
can then only add rights, never remove them — which is consistent with making the whole model
**allow-only**: the effective decision is the union of matching entries, there is no precedence
order to argue about, and no way to write two entries whose combined meaning nobody can predict.
Deny entries are what gets asked for later; they are also exactly what an external policy engine
expresses well ([24](24-opa-security-manager.md)), which is an argument for leaving them out of the
attribute model rather than building a second, weaker policy language.

### The plumbing problem: the PDP cannot see the attributes

`SecurityManager::check(&SecurityContext, ActionType)` receives, for half the variants, only an id:
`GetAgent(AgentId)`, `DeleteSession(SessionId)`, `GetComponent(&str)`. An ACL-evaluating manager
would have to load the entity to read its ACL — but the store is what calls the manager, so the
manager calling the store is a cycle.

- The clean fix is to **pass the attributes in**, by having the store load first and check second,
  with the variant carrying `&SecurityAttributes` or the object itself — which is already how
  `PutPersonality(&Personality)` and `UpdateSession(&Session)` work. It also makes the decision
  better: today `GetAgent(AgentId)` can only be decided on an id.
- A store handle on the manager with an ungated read path works but puts a hole in the store for the
  benefit of the component that is supposed to be guarding it.
- Checking after the load changes what a refusal looks like: `StoreError` distinguishes `NotFound`
  from `PermissionDenied`, and telling the caller which one it was discloses that the object exists.
  Which of the two a denied read returns is a decision, not an implementation detail — and the same
  question decides whether a filtered list is indistinguishable from an empty one.

### Defaults and existing data

Every object in the repository today is constructed with `SecurityAttributes::default()`, i.e.
`owner_id: ActorId::None`. A strict reading of an empty ACL as "nobody" locks out every existing
JSON store on upgrade. An implied owner grant — the owner always holds the instance verbs, the ACL
only adds — avoids that and matches how people expect ownership to behave, but it makes ownership
unrevocable, which has to be a conscious choice.

## Done when

- The set of entities carrying `SecurityAttributes` is decided and the field added, including the
  answer for tools and secrets, which have no entity.
- The duplication between `Metadata.owner` and `SecurityAttributes.owner_id` is resolved rather than
  extended.
- A subject type exists — `ActorId` plus a real `RoleId`, both UUIDs, with a `Role` entity carrying
  the name in its `Metadata` — together with a role assignment that maps actors and backend-supplied
  group names onto roles, carries a `Scope`, and is a permission-protected entity rather than
  backend configuration.
- It is written down that a role holds no rights of its own, and `Scope::GroupScope` is either
  converted to a role scope or removed.
- Role names survive the round trip: hand-written configuration names roles and fails loudly on an
  unknown one, and everything shown to a human resolves ids back to names.
- The verb model is decided, with an explicit answer for `Create` and `List`/`Find`, and a single
  place that maps `ActionType` onto it so that a new action cannot be added without declaring what
  it requires.
- Inheritance and the allow-only rule are written down here.
- `SecurityManager` receives enough to evaluate an entry — i.e. the attributes of the object being
  acted on, not only its id — and a `SecurityManager` implementation that actually reads ACLs exists
  next to `DummySecurityManager`.
- The default for an object with an empty ACL is decided, and existing stores keep working.

## History

- **2026-09-13** — Issue created. The proposal is ACL entries of (subject, actions) on sessions,
  channels, agents, skills and tools; this file widens the entity inventory, records that
  `Scope::GroupScope` and `User.roles` already exist unfinished, and argues that `Create` and `List`
  are scope-level rather than instance-level permissions.
- **2026-09-13** — Subjects decided against groups and in favour of roles, with a separate
  assignment mapping actors and backend-supplied groups onto them. The argument is that the
  directory is not ours: group names cannot be validated when an ACL is written, differ per backend,
  and do not exist under the file backend, whereas roles are defined here and the mapping confines
  deployment-specific naming to one place. Two constraints came with the decision — a role carries
  no rights of its own, and an assignment carries a `Scope` — and the assignment was made a stored,
  permission-protected entity rather than part of a user backend's configuration, since editing it
  grants privilege.
- **2026-09-13** — `RoleId` decided as a UUID, like `ActorId`, with the name held in `Metadata`. The
  case for a readable slug was that ACLs, assignment files and Rego policies are written and
  reviewed by hand, and that `ComponentDescriptor.id` already sets a precedent for stable string
  keys. It lost to renaming: introducing a uniform naming scheme after the fact, or fixing a typo in
  a role that is already referenced, are ordinary housekeeping, and with a name as key both become
  migrations across every reference. The readability that is given up is bought back explicitly —
  configuration names roles and resolves them on load, human-facing output resolves them back, and
  policies match on the id while carrying the name for display.
