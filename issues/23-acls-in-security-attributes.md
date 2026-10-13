# 23 — Access lists: the thing decides, not where one stands

**Status:** done
**Area:** Identity and access management

*Rewritten on 2026-10-10 against what [142](142-roles.md), [253](253-workspace.md),
[255](255-whose-name-agents-act-in.md) and [271](271-publishing-artifacts.md) built; the text it
replaces is in git, and what it had decided and what changed is under `## History`.*

## The problem

`SecurityAttributes` is an `owner_id` and a list of `contributors`, and the ownership policy knows
three answers: the owner does everything, contributors read and use, the system's is everybody's.
That is all one can say about who may reach a thing - and for the two kinds of thing that hold
files, nothing at all:

- **A workspace and an artifact have no contributors** (`validate_users_own`). They are "shared
  through channels": a file of a workspace is read by whoever is a *current member* of a channel of
  the session it is attached to, where the channel's switch is on and the two flow both ways
  (CHANNELS.md, *Its workspace*); a version of an artifact by whoever reads the channel it was
  published in. Outside a channel only the owner reaches a workspace (`GetWorkspace`), and one in
  a session's scope nobody. The REST paths say so: `/sessions/{sid}/channels/{cid}/workspace/…`.
- So the right to a file is a property of **where the caller stands**, not of the file. The same
  file has a different authorization path per channel; a user cleared for the workspace's
  classification cannot download a file through a channel classified below it, though the flow
  rule exists to protect the channel, not the user; a workspace cannot be given to a colleague
  outside any session; and what becomes of an artifact's readers when its channel is deleted, or
  when they leave it, nobody has said.
- **The contributors of a session and a channel are a mirror of their participants**
  (`participant_attributes`) - computed for a channel at every change of its participants, for a
  session once, when it is built from its template, and never again. Everything else sets them
  by hand: an agent contributes to itself, a skill's are written or seeded.
- **Nothing says who may be taken into a session** but the clearance at the door (issue 142):
  whoever may change a session invites whom they like.

## Why it matters

- Working on files is the target of the first release (PLAN.md): people and agents share a
  workspace, and a result is published as an artifact. Who may reach those has to be a rule of the
  thing - readable, settable, lasting - not an accident of membership.
- Every issue that wants to grant something to somebody other than an owner waits for this: an
  administrator ([125](125-invisible-admin.md)), the approval privilege
  ([14](14-approval-privilege.md)), adding participants to a session
  ([221](221-adding-session-participants.md)), which needs a rule for whom.
- It lands before there is production data; stored data may still be deleted (AGENTS.md), so no
  default for existing objects has to be chosen.

## Decisions

Taken with the maintainer on 2026-10-10.

### 1. The thing decides

Whether a caller may reach a workspace, an artifact, a skill, a session is decided by that thing's
`SecurityAttributes` - and by the caller's clearance for its classification - never by the session
or channel the caller happens to act in. "User X is a member of channel Y" is a fact about Y, and
counts whether or not X is in Y at the moment.

So the REST paths that reach a workspace's files and an artifact's versions *through* a channel go.
A channel's view names the workspace (`ChannelView::workspace`, the id), and the client reaches it
at `/workspaces/{id}/…`; an artifact at `/artifacts/{id}/…`. Both answer whoever the access list
admits and whose clearance covers the classification (as `may_see_channel` asks for a channel).

The flow rule between a workspace and a channel (CONFIDENTIALITY.md, *Workspaces*) stays where it
protects the *channel*: an agent using the workspace through its tools in a channel is bounded by
it, as today. Two layers, both asked: the access list says whether one may reach a thing, the
classification whether it may flow to where one stands.

### 2. The attributes

```
SecurityAttributes { owner_id: ActorId, acl: Vec<AclEntry> }
AclEntry { subject: Subject, grant: Grant }
Subject = Actor(ActorId) | Role(RoleId) | MemberOf(Session(SessionId) | Channel(ChannelId))
Grant   = Read | Write
```

- **The owner holds everything, implicitly and unrevocably**; the list only adds. Changing the
  attributes, deleting, reclassifying, attaching a workspace to a session are the owner's alone
  and no grant - the ownership policy as it is.
- **Allow-only.** The effective rights are the union of the matching entries; there is no deny, no
  precedence. Deny is what an external policy expresses well ([24](24-opa-security-manager.md)).
- **`contributors` goes.** What was set by hand becomes `Actor` entries with `Read` (an agent on
  itself, a skill's readers); what was a mirror of membership is not mirrored any more (decision 6).
- **Two grants.** `Read` covers getting, listing, downloading, using (`UseSkill` is granted as
  reading today); `Write` covers changing content - a workspace's files, an artifact's next version,
  a skill's body. Which `ActionType` needs which is one exhaustive mapping
  `ActionType -> (EntityKind, Grant | Owner)`, so that a new action cannot be added without saying
  what it needs. Whether a third grant (`Use`, `Execute`) is ever needed is decided at that mapping,
  not in advance. `Create` stays a question of the scope one creates in, `List` a filter by `Read`
  (how stores check, SECURITY.md).

### 3. Subjects

- **`Actor`** - a user. An agent is never a subject: it acts with its owner's rights (issue 255),
  so an entry is matched against the context's *principal*. An entry naming an agent would let it
  reach what the person responsible for it may not, and is refused when the attributes are
  validated.
- **`Role`** - by a **`RoleId`**, a stable id as everything stored has one: the `roles` section of
  `AppConfig` (issue 142) gives every role an `id` beside its `name` and its clearances, and the
  id is what is stored - in an entry, in `User::roles`, in a personality's limiting roles. What is
  written by hand names roles - the user providers' configuration, a seed - and is resolved to ids
  when it is loaded, failing on a name the configuration does not know; what is shown to a person
  names them again. Both translations are the `SecurityManager`'s (`role_id(name)`,
  `role_name(id)`), which holds the configured roles anyway. A user's roles are read where they
  decide, as for the clearance (issue 181); an agent's are its owner's, **narrowed by its
  personality's limiting roles wherever a role is matched** - an entry of a list, a capability -
  not only at the door as today (`Member::require_cleared`): an agent limited to `staff` does not
  match `Role(hr): Read` on a workspace though its owner holds `hr`. What its owner is granted as
  an `Actor` stays the agent's too; who wants to keep it from the agent grants the role, not the
  person. A bound that only takes, never gives - an IAM permission boundary in small. This is the
  one way an agent may do less than its owner; it settles [181](181-where-roles-decide.md)'s
  question of where roles count: wherever an entry names one. An entry naming an unknown role is
  refused when it is written.
- **`MemberOf`** - the current participants of a session or a channel: users, and agents through
  their owner - *or* as the caller, for what an agent takes part in itself (issue 264). This is the
  entry that replaces "shared through channels": a workspace made for a session carries
  `MemberOf(session)`, an artifact published in a channel `MemberOf(channel)`.
- **Not a subject: a group of the directory** (LDAP, a user file's groups). The reason of 2026-09-13
  stands: the directory is not ours, its names cannot be validated when an entry is written and
  differ per backend; they map onto roles in one place, the user provider. **A group of our own** -
  a team, an entity with members that users make and manage - fits the same slot and is wanted for
  sharing a workspace with "the project"; it is an entity with ownership and scope of its own and
  comes as an issue of its own, after this one.

### 4. Rights of a role

A right *on a thing* lives in the thing's list; a right *without a thing* - creating a session, a
personality, a template; reading what was deleted and purging it, for an administrator
([125](125-invisible-admin.md)); whatever [181](181-where-roles-decide.md) finds should ask for a
role - has no object to live on and belongs to the role, in the configuration, beside the
clearances it grants already:

```yaml
roles:
  - id: …
    name: admin
    clearances: [ … ]
    capabilities: [create_session, create_personality, read_deleted, purge]
```

One fact has one home: "role r may read workspace Y" is Y's entry `Role(r): Read`, never a line
of r's. The mapping of decision 2 then says of every action what it needs: `Grant | Owner |
Capability(…)`. A capability bounded to a scope ("may create sessions in …") can be a field later;
the first release needs none.

AWS IAM does it the same way: a *resource-based policy* on the thing (a bucket policy) and an
*identity-based policy* on the identity - and `CreateBucket` is only ever the identity's, on
`Resource: "*"`. Both are evaluated as a union without precedence, as this list is; the cost that
"why may X" has two places to look is paid there with tooling, here with a refusal that names what
was missing (`ActionType::describe`). What AWS calls a *role* is an identity one assumes - closer to
an agent acting as its owner (255) than to a role of ours, which is an IAM *group*. Not built now;
written down so that the configuration and the mapping have the shape.

### 5. The session's admission rule

A session carries, besides its `acl`, an **`admission: Vec<Subject>`**: who may be taken in after it
was built. It comes from the template (`SessionTemplate::admission`, written by whoever may change
the template) when the session is built; the template's own participants are taken in as the
template says and are not asked. Everyone added later - `add_participant`, and whatever
[221](221-adding-session-participants.md) builds - must match it, *and* pass the clearance at the
door as today. **Empty means nobody**: a session whose template says nothing takes nobody in later.
**The owner may narrow it, never widen it**: `UpdateSession` admits a new `admission` only if every
subject of it was in the old one. A channel takes in only who the session took in, as now.

### 6. Membership is the policy's to check

An entry `MemberOf(…)` names one session or one channel; whether the principal (or the caller) is a
participant of it is a fact the `SecurityManager` establishes itself, not something the caller
passes in - the caller cannot know which places a list names. The `OwnershipSecurityManager` takes
a **`Memberships`** source when it is made, as it takes the `ConfidentialityRules`:

```
trait Memberships { async fn is_member_of(&self, actor: ActorId, place: Place) -> bool; }
Place = Session(SessionId) | Channel(ChannelId)
```

The runtime implements it over the stores in the system's name, reading the one session or channel
the entry names - never scanning. It is asked only when a list holds such an entry. A session's and
a channel's own readers are answered the same way: their lists hold `MemberOf(self)` rather than a
mirror of their participants, so that membership has one source and a session's list does not
stop at its creation.

- **Reentrancy.** A store checks after it has loaded, with the value in hand and no lock held (the
  memory store's `std::sync::RwLock` guard cannot live across the `await`); the manager reading
  another entity through the store is then a second, independent call. To be verified for every
  path when step 1 is built, lists and the JSON store included.
- **A cache** of memberships, fed by the participant events, is an optimisation and an issue of its
  own; correctness first.
- A denied read stays "not found", as today, and a filtered list is indistinguishable from an
  empty one.

### 7. Defaults

| Thing | List when made | Who adds |
|---|---|---|
| Workspace in a session's scope | `MemberOf(session): Read, Write` | - |
| Workspace of a user's | empty; attaching it to a session shares nothing | the owner adds `MemberOf(session)` as the explicit act of sharing - offered where it is attached |
| Artifact | `MemberOf(channel it was published in): Read` | the owner |
| Skill | its readers as `Actor: Read` (seed, `create_skill`) | the owner |
| Session, channel | `MemberOf(self): Read` | - |
| Agent | empty: it reads its own definition as its owner (issue 255) | the owner |
| Session's `admission` | the template's | the owner narrows |

### 8. What this does not change

- `CallTool`, `ReadSecret` and the credentials of components stay as they are (issue 255: the
  owner's); an access list on a component descriptor can follow when something needs it.
- The confidentiality actions and the flow rule (CONFIDENTIALITY.md).
- Roles of an agent's own ([181](181-where-roles-decide.md), 179).
- Stored data: no compatibility. Stores with sessions, channels, agents, skills, workspaces and
  artifacts are deleted; the seed is adapted.

## Plan

Each step reviewable on its own; the first changes nothing in who may do what.

1. **Restructure.** `RoleId` in the `roles` section, `User::roles` and a personality's limiting
   roles as ids, the user providers and the seed naming roles and resolving them on load, the
   `SecurityManager` translating; `SecurityAttributes { owner_id, acl }`; `Memberships` and the
   policy evaluating `Actor`, `Role` and `MemberOf`; the mapping
   `ActionType -> (EntityKind, Grant | Owner)`; sessions
   and channels with `MemberOf(self)`, agents and skills with `Actor` entries; validation (no agent
   as a subject, known roles); the security suite as it is, passing.
2. **Workspaces.** `/workspaces/{id}/…` for whoever the list and the clearance admit; the channel
   routes removed, `ChannelView::workspace` the id; defaults of decision 7; the flow rule kept for
   the tools; the web UI following; the suite extended.
3. **Artifacts**, likewise, with `MemberOf(channel)` on publishing.
4. **Admission.** `SessionTemplate::admission`, `Session::admission`, checked in `add_participant`;
   the owner narrowing only; the seed's templates say whom.
5. **Documents.** SECURITY.md (*Ownership* becomes *Access lists*), CHANNELS.md, CONFIDENTIALITY.md
   (*Workspaces*), REST-SERVER.md, WEB-UI.md, STORAGE.md; which stored data is deleted.

Issues to open with it: a group of our own as a subject; a cache for `Memberships`.

## Done when

- `SecurityAttributes` holds an access list of `Actor`, `Role` and `MemberOf` entries with `Read`
  and `Write`, allow-only, the owner implied; `contributors` is gone.
- Roles have an id in the configuration; stored roles are ids, hand-written ones names, and the
  `SecurityManager` translates both ways.
- A workspace's files and an artifact's versions are reached by their own routes, by whoever the
  list and the clearance admit, never through a channel; the channel routes are gone.
- A session has an admission rule from its template, empty for nobody, that its owner can narrow
  and not widen, checked for everyone taken in later.
- The policy answers `MemberOf` through a `Memberships` source of the runtime, reading the one place
  named; the reentrancy of the stores is verified.
- The security suite covers reading a workspace and an artifact as a member, a stranger, and a
  member of another channel of the session; narrowing and widening the admission; an entry naming
  an agent refused.
- The documents of the plan's step 5 describe it.

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
- **2026-10-10** — Rewritten with the maintainer, now that a concrete need exceeds owner and
  contributors: workspaces (253) and artifacts (271) are reached only through a channel, and the
  right to a file should be the file's. What changed against the text of 2026-09-13: a role keeps
  its `RoleId`, but as a field of the configuration (142 configures roles there), not of a `Role`
  entity, and there is no stored assignment - the user provider gives users their roles and maps
  the directory's groups; the `SecurityManager` translates ids and names; a role does carry
  something now, its clearances; a group of the directory is still no subject, a group of our
  own is to come as an issue of its own; `contributors` becomes the list rather than standing beside
  it; membership of a session or channel is a subject, established by the policy through a
  `Memberships` source that reads the one place named - a cache is an optimisation for later; the
  grants are `Read` and `Write`, the rest the owner's; a session gets an admission rule from its
  template, empty for nobody, which its owner may only narrow; agents are never subjects (255),
  and a personality's limiting roles narrow an agent wherever a role is matched, the one way it
  does less than its owner; a right without an object - creating, administering - is a capability
  of a role in the configuration, not built yet. Branch `issue-23-acls`.
- **2026-10-10** — Step 1 done, without a change in who may do what - the security suite passes
  as it was, and gains a test of its own. Built:
  - `caelumora_model::role`: `RoleId`, `Roles` (moved out of `confidentiality`, each role an `id`,
    a `name`, its clearances), `user_roles`. `User::roles`, `Personality::roles` and
    `JoinChannel` hold ids; `accept_any`'s configuration and a seed's personalities name roles,
    resolved when they are loaded (`AppConfig::role_ids`, `SeedPersonality`); the
    `SecurityManager` translates (`role_id`, `role_name`), the policy that allows everything too.
  - `SecurityAttributes { owner_id, acl }` with `AclEntry`, `Subject`, `Membership`, `Grant`;
    `contributors` and `participant_attributes` are gone. A session and a channel carry
    `MemberOf` themselves; an agent carries nothing - the table of decision 7 said
    `Actor(itself)`, which decision 3 forbids and which 255 made needless. The stores refuse an
    entry naming anyone but a user (`SecurityAttributes::validate`).
  - The ownership policy evaluates the list (`granted`), learning memberships and roles from a
    `Memberships` it is made with; `StoreMemberships` reads the one place, user or agent asked
    about, a deleted session or channel too, holds the store weakly and is bound by the binaries
    once the store exists. The policy's `match` stays the one exhaustive mapping of decision 2;
    no action asks for `Write` yet.
  - A participant removed from a channel is no member when its removal is announced, so the
    announcement is delivered with an entry for them besides, never stored.
  - The terminal user no longer holds the role "user", which was configured nowhere.
  Stored data to delete: every entity store - users, personalities, sessions, channels, agents,
  skills, workspaces, artifacts. A configuration's `roles` need an `id` each.
- **2026-10-10** — Step 2 done, decided with the maintainer before it was built:
  - `WriteWorkspace` beside `GetWorkspace`: reading a workspace's files, the tools that only read
    and listing are `Read`; uploading, deleting, the tools that write and every shell tool -
    a command may change anything - are `Write`. `UpdateWorkspace` stays its owner's, sharing
    included (`WorkspaceManager::set_access`, `PUT /workspaces/{id}/access`).
  - A person reaches the files by the workspace, wherever they act (`WorkspaceManager::reached`,
    `WorkspaceView::terminal(Grant)`): the list and their clearance; the scope no longer keeps a
    workspace made for a session from being reached outside it. The four channel routes of the
    workspace are gone; a channel's snapshot names the workspace by `workspace_id` where it uses
    it and the viewer may read it. `GET /workspaces` lists what is shared with the user too.
  - An agent still uses it through a channel - switch, flow rule -, and now as the list lets it
    besides (`usable_in` asks `GetWorkspace`; changing asks `WriteWorkspace`, `ReadOnly`
    otherwise); the prompt part says when it may only read.
  - One made in a session's scope carries `MemberOf(session): Write` (`Workspace::new`). Attaching
    one of a user's shares nothing unless `share` says `read` or `write`
    (`SessionLoop::set_workspace`, `WorkspaceChoice::Existing`, the web UI's `ShareSelect`);
    detaching leaves the list. Comparing the session's rules with the workspace's when attaching,
    to spare entries for single sessions, is issue
    [289](289-attach-compares-access.md).
  - Deviation from what was agreed: tools are not offered by what the list grants -
    `ToolProvider::tools` knows no turn, and even "no workspace in this channel" is answered at the
    call today -, so a tool that changes fails at the call where the list grants reading. Offering
    by the turn is a change of `ToolProvider` of its own.
  - An artifact keeps an empty list until step 3 (`Artifact::validate`).
  Stored data to delete: workspaces, whose attributes now carry the list.
- **2026-10-10** — Step 3 done, decided with the maintainer before it was built. A list on the
  artifact alone (decision 7's `MemberOf(channel it was published in)`) would have let the members
  of every channel a version was published in read all versions - drafts published where few are
  among them. Instead, beside the artifact's own list, **every version carries a list of its own**
  (`ArtifactVersion::acl`), which publishing gives the members of its channel; a version is read by
  whom either grants it (`Artifact::version_attributes`, asked as `GetArtifact`), so a draft kept
  to a private channel reaches nobody who reads the version published to the team. One meaning
  for an entry everywhere: it grants what it hangs on.
  - The description a publication gives goes to its version; the artifact's own is its owner's,
    set by `describe`. Whoever reads a version sees the artifact's name and its owner's
    description, and of the versions only those they may read - their numbers show that others
    exist, accepted.
  - Adding a version is `WriteArtifact`, what the artifact's list grants `Write` - its owner
    always; a version's list grants reading alone (`Artifact::validate`). The artifact is changed
    in the runtime's name then, its owner unchanged. Keeping, describing, sharing
    (`ArtifactManager::set_access`, `PUT /artifacts/{id}/access`) and deleting stay its owner's.
  - Reading goes through the manager (`readable`, `reached`, `all_readable`, `published_in` with
    the viewer's context), which reads in the runtime's name and asks per version - the store's
    own check knows the artifact's list alone. `content` asks the clearance besides. Events about
    a version go to its readers, those about the artifact to all who read any version
    (`reader_attributes`).
  - The channel's download route is gone; `GET …/channels/{cid}/artifacts` stays, listing what
    was published there that the viewer may read. Showing and changing whom an artifact is shared
    with in the web UI is issue [291](291-share-artifacts-in-the-web-ui.md); changing a version's
    own list after publishing is not offered yet.
  - Names of whom a list names are resolved in one place for workspaces and artifacts
    (`Views::access_snapshots`).
  Stored data to delete: artifacts, whose versions now carry a list and a description.
- **2026-10-10** — Step 4 moved to issue [221](221-adding-session-participants.md), with the
  maintainer: nobody is taken into a session after it was built today - a channel takes in only
  the session's participants, which its template fixes -, so an admission rule would have had
  nothing to check. Decision 5 stands and is built with 221, whose *Done when* it now carries.
  Step 5 was done along the way: the documents were brought up to date with each step
  (SECURITY.md's *Access lists*, CHANNELS.md, CONFIDENTIALITY.md, REST-SERVER.md, WEB-UI.md,
  TOOLS-AND-COMMANDS.md, CONFIGURATION.md, SKILLS.md, ARCHITECTURE.md). Closed: the attributes
  hold an access list of users, roles by id and members of a place, allow-only, the owner
  implied; a workspace's files and an artifact's versions are reached by their lists, never
  through a channel; the policy answers membership through `Memberships`, reading the one place
  named; the security suite covers members, strangers, readers and writers, drafts kept to their
  channel and an entry naming an agent refused. Left: the admission rule (221), groups of our own
  (286), a cache of memberships (287), comparing access when attaching (289), tools offered by the
  turn (290), sharing artifacts in the web UI (291), the capabilities of a role (decision 4, with
  125 or 221). Stored data to delete over all steps: every entity store; a configuration's roles
  need an id each.
- **2026-10-13** — The decision of step 3 that a version is read by whom the artifact's list *or*
  the version's grants it was revised in issue
  [316](316-store-checks-artifact-without-versions.md): reading a version needs both, as in HDFS
  and ADLS, and publishing lets the channel's participants read the artifact as well.
- **2026-10-13** — Decision 5 was replaced in issue
  [221](221-adding-session-participants.md): whom a session or channel lets in is held in its
  access list as `Join` entries, with a subject for everyone, rather than in an `admission` list
  beside it; the owner's narrowing went with it, since inviting is widening. The capabilities of a
  role (decision 4) stay open.
