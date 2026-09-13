# 23 — ACLs in `SecurityAttributes`

**Status:** open
**Area:** Identity & access management

## The problem

`SecurityAttributes` holds exactly one field, `owner_id: ActorId`, and nothing
reads it ([11](11-agent-ownership.md)). There is no way to express that a
second user may read a session, that a group may start sessions from a
template, or that one agent — and only that one — may execute a given tool.
Access is therefore all-or-nothing per deployment, and with
`DummySecurityManager` the answer is always "all".

What is missing is the *data*: an access control list on the objects being
protected, saying which subject may perform which kind of operation. The
decision itself stays where it is — `SecurityManager` remains the single
policy decision point; ACLs are only the attributes it reads.

Three things have to be decided together, and the third is the one with no
obvious answer:

1.  **Which entities carry `SecurityAttributes` at all.**
2.  **What a subject is** — an `ActorId`, or a group, of which there is none.
3.  **What an entry grants.** `Create`, `Read`, `List`/`Find`, `Write`,
    `Delete`, `Execute` is the obvious verb set, but not every verb means
    anything for every entity kind, and a verb set that is two thirds
    inapplicable is a sign that the model is wrong rather than merely
    incomplete.

## Why it matters

Every issue in this area is blocked on the absence of this data.
[10](10-user-authentication.md) produces an authenticated identity with
nothing to check it against. [12](12-user-and-agent-permissions.md) asks how
to combine two sets of rights that do not exist. [14](14-approval-privilege.md)
needs to say who is entitled to approve. [22](22-skill-management.md) needs to
say who may load which skill. Each of them can be answered once there is a
place to write the answer down, and none of them before.

It also matters that this lands *before* there is production data. Retrofitting
an ACL onto stored entities means choosing a default for every existing object,
and every default is wrong for somebody.

## Considerations

### Which entities need `SecurityAttributes`

Today the field exists on `Agent`, `Session` and `Channel`, seeded from
`SessionContext::owner` when a template is instantiated. The proposal adds
skills and tools. The full inventory is larger:

- **`Session`, `Channel`, `Agent`** — already have the field, nothing reads it.
- **`Personality`** — defines what an agent *is*; whoever may edit a
  personality controls every agent instantiated from it. Needs both an edit
  right and an instantiate right, and those are not the same right.
- **`SessionTemplate`** — same argument: starting a session from a template is
  the interesting operation, editing it is the dangerous one.
- **`ComponentDescriptor`** — preconfigured components are referenceable from
  anywhere via `kind: "ref"`, and an `mcp_tool` descriptor names a secret. A
  component descriptor is the most privileged object in the store and the one
  with no attributes at all today.
- **`User`** — who may read, list and update a user; and, once groups exist,
  who may change a user's membership.
- **`Group`** — does not exist yet (below), and will need attributes of its
  own; membership management is itself a privileged operation.
- **`Skill`** ([22](22-skill-management.md)) — the loading right is the whole
  point of that issue.
- **Tools** are the awkward case, because a tool is not a stored entity. Tools
  are produced by `ToolProvider` components at build time, and an `mcp_tool`'s
  list is discovered from a remote server at runtime — so no ACL written in
  advance can enumerate them. Either the ACL lives on the
  `ComponentDescriptor` and applies to everything that component exposes
  (coarse, but it matches where the credential already sits), or tool entries
  are name patterns (`github-mcp__*`), which is the same namespacing convention
  `mcp_tool` already uses. Per-tool entries for tools that do not exist until
  connect time cannot be validated at authoring time either way.
- **Secrets** are worse: `ActionType::ReadSecret(&str)` names a key, and a key
  is a string in a provider, not an object. Either secrets get a descriptor
  entity that can carry attributes, or the ACL is part of the provider
  configuration. This should be decided here rather than left to
  [12](12-user-and-agent-permissions.md), which assumes the answer.
- **State** (`AgentState`, `SessionState`, `ChannelState`) deliberately gets
  nothing of its own: `StateStore` does not even take a `SecurityContext`
  today, and state belongs to exactly one entity. It should inherit, not carry
  a second copy that can drift.
- **Messages and history** likewise. Access to messages follows channel
  membership; a per-message ACL would be a weaker duplicate of it.

### Ownership is already stored twice

`Metadata` carries `owner: UserId` *and* `scope: Scope`; `SecurityAttributes`
carries `owner_id: ActorId`. They can disagree, the types differ (an agent can
own something, a `Metadata.owner` cannot), and `Channel` has
`SecurityAttributes` but no `Metadata` at all while `Personality` has
`Metadata` but no `SecurityAttributes`. Adding an ACL next to a duplicated
owner field makes three sources of truth. The overlap has to be resolved
first — most likely by `Metadata` keeping name, description and scope, and
everything security-relevant moving into `SecurityAttributes`.

### What a subject is

Three subject vocabularies are already half-started in the code and none is
finished:

- `ActorId` — real, used everywhere, covers users and agents.
- `Scope::GroupScope { group_id: Uuid }` — a group id in the scope hierarchy,
  with no `Group` entity, no `GroupId` newtype and no membership anywhere.
- `User.roles: Vec<String>` — exists on every user and is read by nothing.

Shipping ACLs on top of all three would give the deployment two independent
ways to say the same thing. The recommendation is one subject enum over
`ActorId` and a new `GroupId`, with `User.roles` either folded into groups or
deleted — a role that is just a group with a name is not worth a second
mechanism. Groups then need a real entity with membership, their own
attributes (recursion: who may add a member), and a membership resolution that
is cheap, because it is on the path of every single check. Whether groups may
nest, and what happens to a cycle, is part of that.

### What an entry grants — and why the verb list is the hard part

`Create` / `Read` / `List`+`Find` / `Write` / `Delete` / `Execute` applied
uniformly to every kind is the simplest thing that could work, and its problem
is visible immediately: `Execute` on a `Session` means nothing, `Create` on an
existing `Agent` means nothing, and an ACL editor would offer both.

Options:

- **A verb enum per entity kind.** Type-safe and self-documenting, but
  `SecurityAttributes` is one struct shared by all kinds, so it would need a
  type parameter or a per-kind variant, and every store, serializer and API
  gains a case per kind. The safety is real; the cost is spread over the whole
  codebase.
- **A single generic verb set, with nonsensical combinations simply never
  granted.** Cheap and uniform, and wrong entries are inert rather than
  dangerous — but nothing stops them being written, and "inert" is only true
  as long as no future verb reuses the name.
- **Derive the verbs from `ActionType`.** This is the option worth taking
  seriously, because the enumeration already exists and is already the
  authoritative list of securable operations: `GetPersonality`,
  `PutSessionTemplate`, `CallTool`, `ReadSecret` and the rest. Instead of
  inventing a second vocabulary that has to be kept in sync by hand, add one
  total function `ActionType -> (EntityKind, Verb)`: a single exhaustive
  `match`, so the compiler forces every new `ActionType` variant to declare
  what it is. ACL entries stay generic, and `Verb::applies_to(kind)` — derived
  from the image of that mapping — lets the authoring path reject `Execute` on
  a session instead of storing it.

Independently of which is chosen, two of the proposed verbs are not instance
permissions at all:

- **`Create` has no instance to hang on.** The object does not exist when the
  check runs, so the permission is really "may create an object of this kind
  *here*" — a property of the container, not of the created object.
- **`List` / `Find` is elementwise `Read`.** If listing were a separate grant,
  `list_personalities` would have two answers that can disagree. The
  defensible semantics is that a list returns exactly what the caller may
  read, which makes `List` a filter rather than a verb — at the cost of N
  checks per call, and that cost is the reason the alternative keeps getting
  proposed. Once there is a Postgres store ([03](03-postgres-entity-store.md))
  the filter wants to be a query predicate rather than a post-filter, which
  argues for keeping the ACL shape simple enough to translate into SQL.

So the model that falls out is: **instance permissions** (`Read`, `Write`,
`Delete`, `Execute`/`Use`) on the object, and **collection permissions**
(`Create`, and enumeration if it stays separate) on a *scope*. `Scope` already
exists, already forms a hierarchy and already has `contains()` — it is the
natural home for the second kind, and it gives inheritance for free.

### Inheritance, and allow-only

If a `Channel` inherits its `Session`'s entries, a session template does not
have to restate an ACL per channel, and containment already has a meaning via
`Scope::contains`. The price is that a child can then only add rights, never
remove them — which is consistent with making the whole model **allow-only**:
the effective decision is the union of matching entries, there is no
precedence order to argue about, and no way to write two entries whose
combined meaning nobody can predict. Deny entries are what gets asked for
later; they are also exactly what an external policy engine expresses well
([24](24-opa-security-manager.md)), which is an argument for leaving them out
of the attribute model rather than building a second, weaker policy language.

### The plumbing problem: the PDP cannot see the attributes

`SecurityManager::check(&SecurityContext, ActionType)` receives, for half the
variants, only an id: `GetAgent(AgentId)`, `DeleteSession(SessionId)`,
`GetComponent(&str)`. An ACL-evaluating manager would have to load the entity
to read its ACL — but the store is what calls the manager, so the manager
calling the store is a cycle.

- The clean fix is to **pass the attributes in**, by having the store load
  first and check second, with the variant carrying `&SecurityAttributes` or
  the object itself — which is already how `PutPersonality(&Personality)` and
  `UpdateSession(&Session)` work. It also makes the decision better: today
  `GetAgent(AgentId)` can only be decided on an id.
- A store handle on the manager with an ungated read path works but puts a
  hole in the store for the benefit of the component that is supposed to be
  guarding it.
- Checking after the load changes what a refusal looks like: `StoreError`
  distinguishes `NotFound` from `PermissionDenied`, and telling the caller
  which one it was discloses that the object exists. Which of the two a denied
  read returns is a decision, not an implementation detail — and the same
  question decides whether a filtered list is indistinguishable from an empty
  one.

### Defaults and existing data

Every object in the repository today is constructed with
`SecurityAttributes::default()`, i.e. `owner_id: ActorId::None`. A strict
reading of an empty ACL as "nobody" locks out every existing JSON store on
upgrade. An implied owner grant — the owner always holds the instance verbs,
the ACL only adds — avoids that and matches how people expect ownership to
behave, but it makes ownership unrevocable, which has to be a conscious
choice.

## Done when

- The set of entities carrying `SecurityAttributes` is decided and the field
  added, including the answer for tools and secrets, which have no entity.
- The duplication between `Metadata.owner` and `SecurityAttributes.owner_id`
  is resolved rather than extended.
- A subject type exists — `ActorId` plus a real `GroupId` with a `Group`
  entity and membership — and `User.roles` is either folded into it or
  removed.
- The verb model is decided, with an explicit answer for `Create` and
  `List`/`Find`, and a single place that maps `ActionType` onto it so that a
  new action cannot be added without declaring what it requires.
- Inheritance and the allow-only rule are written down here.
- `SecurityManager` receives enough to evaluate an entry — i.e. the attributes
  of the object being acted on, not only its id — and a `SecurityManager`
  implementation that actually reads ACLs exists next to
  `DummySecurityManager`.
- The default for an object with an empty ACL is decided, and existing stores
  keep working.

## History

- **2026-09-13** — Issue created. The proposal is ACL entries of
  (subject, actions) on sessions, channels, agents, skills and tools; this
  file widens the entity inventory, records that `Scope::GroupScope` and
  `User.roles` already exist unfinished, and argues that `Create` and `List`
  are scope-level rather than instance-level permissions.
