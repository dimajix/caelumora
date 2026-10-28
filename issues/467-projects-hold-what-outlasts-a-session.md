# 467 — Results have no place of their own: a project holds what outlasts a session

**Status:** in progress
**Type:** feature
**Priority:** high
**Area:** Sessions and channels
**Summary:** An artifact lies in the scope of the session it was published in and goes with it, because nothing above a session holds what is meant to outlast one piece of work.

## The problem

Files have a container: the workspace, an entity with an owner, a storage, a lifetime and a place
it is found by name. Artifacts have none. Each artifact lies in a `Scope` by itself, and that
scope has to do three jobs at once - be the name space, decide the lifetime, say where the artifact
is found -, with one value. That is where the questions of issue
[338](338-artifacts-go-with-their-session.md) come from: which scope an artifact is published into,
a `keep` per artifact to move it out of the session before the session goes, and names that would
collide across sessions once an artifact lies in its owner's scope. Choosing a scope answers none
of them well; what is missing is the container.

Caelumora has two brackets, the session and the channel, and both are about one interaction. There
is no bracket for what outlasts it: the results of several sessions on one subject, found by name,
read by those who work on that subject, kept as long as the subject is worked on.

## The concept

A **project** is that bracket. The three brackets then have one purpose each:

| Bracket | What it is for | What belongs to it |
|---|---|---|
| **Project** | what outlasts one piece of work: its results, and the name space they are found in | the artifacts; its sessions; later what else a subject shares - skills, personalities, a workspace |
| **Session** | one piece of work: who takes part, how private it is, its channels, the room it is done in | its workspace, local or from elsewhere; its agents; what is made for it alone |
| **Channel** | one conversation: its classification, its history, who hears what | its messages and notes; who reads a version of an artifact |

A workspace and a project are both places things lie, and stay apart: the workspace is the room the
work is done in, bound to a session as a rule and shared by several at times; the project is where
the results go.

### The project

A project is a stored entity of the core, `Project`, owned by a user, with a name, a description,
members and an access list. It lies in a scope - the global one, its owner's, or a session's;
a group's is the later step with issue [340](340-teams-as-owners.md), once the group's scope is
a place anything is found from -, where it is found by name; projects in one scope have
different names. And it
**holds a scope of its own**, `Scope::ProjectScope { project_id }`: what lies in it belongs to the
project. That is the whole mechanism. Everything the scopes already do holds for a project's:
names are unique in it, what lies in it goes with its holder (`ScopeMember`, issue
[178](178-scoped-lookup-by-name.md)), in the name of whoever deletes the holder, and
`ScopeNotEmpty` if something is left (issue [320](320-scoped-deletion-in-the-runtimes-name.md)).
Artifacts are the first kind that lies in a project; skills, personalities and workspaces can
follow without a new mechanism. The project chooses no storage of content - the section
`artifacts` does, as today -, unlike a workspace, whose storage is its own.

**Members.** A project has members, as a session has participants: the users who work on the
subject. Their group is kept as the participants of a session are (issue
[348](348-participants-groups-out-of-step-with-their-holder.md)): `GroupId::of_project`, made
with the project, kept up to date as members change, deleted and restored with it, with an id of
its own. The group is a subject an access list can name without naming each member: the project's
own list names it for reading, as a session's names its participants, and an artifact published
into the project is read by it. Members read the project, make sessions in it and publish into it.
The access list, besides, shares the project with whom the owner names - a team that sees it and
its artifacts.

Sub-projects are not done: a project lies in no other project's scope. A project is not a
`ScopeMember` of the scopes it may lie in, but one of them: see *A session's own project*.

### Every session has a project

`Session::project: ProjectId` names the project a session's results go to, always: there is no
session without one. Artifacts lie in a project's scope and nowhere else.

**A session's own project.** A session is made with a project of its own, lying in the session's
scope, as it is made with the group of its participants: kept by the session - deleted, restored
and purged with it, by the session itself, not through `ScopedEntities` -, and not deleted on its
own, since it is the session's. Its list of members is empty: who reads the session reads its
results through the channels they take part in, as today. A session that lies in no shared
project publishes into its own.

**A session in a shared project.** A session lies in the scope of its project where that is not
its own: made in the project, or put into it later. It goes with the project, as everything in a
scope goes with its holder: `SessionManager` is a `ScopeMember` of project scopes. There are these
two states and no third - a session does not use a project it does not lie in.

Whoever may change the session (`UpdateSession`) and read the project (`GetProject`) puts a
session into a project, or takes it out, in which case it publishes into its own project again -
the one in its scope, or a new one if it was lifted. **Nothing moves**: what was published into
the former project stays in it, what lies in the session's own project stays there as long as the
session does; only new publications go to the new one. The agents of the session keep the
personalities they were made with, its workspace stays attached; what a session finds by name
from its place changes with the next lookup alone (see *Lookups from a place*).

**The project is shared with the session's participants.** A session that lies in a project
shares it with the group of its participants for reading, as a workspace attached to a session is
shared with them (`SessionManager`, issue [253](253-workspace.md)): so every participant may
publish into it, member or not. The session's own project is shared so when the session is made -
whoever reads the session reads its project -, which makes it one rule for every project. Taking
a session out of a project takes the entry away, since the group lives on; a deleted session's
goes once the group is gone (issue [347](347-entries-naming-purged-groups.md)).

**Lifting.** The one act of keeping: the owner of a session's own project lifts it into their
scope or the global one (`UpdateProject`, a change of the project's scope). It is then a shared
project like any other, with everything that lies in it - the session, which now goes with it,
and its artifacts -, and the owner names who its members are. Nothing is moved, no version
history is lost, and no artifact is kept by itself: the `keep` of an artifact, its
`PUT /artifacts/{id}/scope`, `ArtifactError::InvalidScope` and `ArtifactUpdateEvent::ScopeChanged`
go, and with them the buttons to keep in the web UI. Moving a single artifact into another project
is not done; the mechanism is here should somebody need it.

### Artifacts

An artifact is published into the scope of its session's project, found by name there: the
artifact of that name gains a version, or is made with its first, as today within the session -
one not the caller's to add to is refused as such (`WriteArtifact`). The project is the name
space: a `report.csv` of one project and one of another are two artifacts. So the question of
issue 338, how a version is added to another's artifact once names are per owner, does not arise.
An artifact is made in a project's scope only by who may read the project, as a workspace is made
in a session's scope only by who may read the session; the session's participants may, through
the share the session brought.

**Who reads it.** Publishing grants the participants of the channel it was published in reading
of the artifact and of the version, as today (issues [23](23-acls-in-security-attributes.md) and
[316](316-store-checks-artifact-without-versions.md)), and the members of the project reading of
the artifact alone: a draft published where few are reaches nobody else, and a member who took no
part in that channel sees that the artifact is there, and the versions published where they
were. For a session's own project the members' group is empty, and the entry grants nobody
anything until the project is lifted; publishing has no case for it.

**Lifetime.** An artifact lives as long as its project: with a session's own project, as long as
the session, now by design - a session that lies in no project is work done for its own sake, and
whoever wants results kept works in a project, or lifts the session's own; in a shared project,
beyond every session. `ArtifactManager` stays a `ScopeMember`, of project scopes;
`Artifact::validate` accepts a project's scope and no other.

### Where the groups of a session stand

The group of a session's participants stays in every list it is in today, since each is about the
session itself: the session's own list; a workspace made in the session's scope, which stays the
session's room; a workspace attached to the session, shared with it as its owner chose. The
project's group is named where the results are: on the project's own list and on every artifact
published into it; and the session's group is named on the project the session lies in. The REST
API's `AccessKind`, which names a session's or a channel's participants in a share, names a
project's members too. Two sessions of one project do not share a workspace by that: a workspace
lying in a project's scope, used by all its sessions, is the later step *workspaces in projects*.

### Lifecycle of a project

A `ProjectManager` owns it, as the other managers own their kinds (`create`, `delete`, `restore`,
`purge`, issue [354](354-names-follow-several-schemes.md)): deleting a project deletes what lies
in its scope in the deleter's name through `ScopedEntities` - its sessions, each with what lies in
its scope, and its artifacts -, as deleting a session does, and refuses with `ScopeNotEmpty` if
something is left the deleter may not delete; restoring restores what the caller may; purging
purges all of it, the artifacts' content included. Its members' group goes with it. A session's
own project is not deleted by the manager: it is the session's, and goes with it.

### Lookups from a place

Three kinds of lookup by name exist, and only one depends on where it is made from:

- Agents and workspaces a session template names are found once, when the session is made, in
  exactly the scope the template says (`template.rs`); the id is stored, and a project change
  touches nothing.
- A bundle reference names its scope itself (`ComponentRegistry::resolve`).
- The catalogue of skills lists what is seen from the channel at every use
  (`Filter::seen_from`, `found_from`, issue [293](293-store-queries.md)), and derives the scopes
  around the place from the scope value alone (`Scope::holders`).

A session's scope does not know its project, and must not: scope values are stored, and sessions
change projects. So `ProjectScope` contains nothing structurally but itself, and the chain of
scopes around a place - channel, session, the session's project, the global scope - is given by
whoever knows the session, not derived from the value. That is needed once skills or
personalities lie in projects, not for artifacts, which nothing looks up from a place; it is a step
of its own, after this issue. Until then, what lies in a project's scope is not found by name from
a session, and `Scope::usable_at` does not answer for it.

## Decisions

Taken with the maintainer on 2026-10-28, while looking at issue 338:

- A project is a scope, not a new mechanism; artifacts are the first kind to lie in one, from the
  plugin, which takes the scope up as it takes up the session's.
- Artifacts stay entities with their own access list and reading per version, lying in the
  project - not content of the project behind a storage of its own.
- A `Project` entity: description, access list, members. No storage is bound to a project.
- A session has a project, always - its own, lying in its scope and kept by the session as its
  participants' group is, or a shared one it lies in. Two states, no third: a session does not use
  a project it does not lie in.
- Projects live longer than their sessions: a session in a project goes with it.
- Changing a session's project moves nothing; what lies in the session's own project stays as
  long as the session does. A session may change its project; what it finds by name changes with
  the next lookup, nothing stored is looked up again.
- Keeping is lifting the session's own project; no artifact is moved by itself.
- Who may read a project may publish into it, the pattern of the workspace made in a session's
  scope; the session's participants read it through the share the session brought.
- Members are a list of the project's own, with a group of their own - not the owner of the scope
  the project lies in: a shared private project would need a team, a global one would have nobody.

## What it replaces

- Issue [338](338-artifacts-go-with-their-session.md) is superseded: its proposal - an artifact in
  its owner's scope, a version added by id - is not done; `keep` goes, as it proposed, since
  lifting takes its place. Its bug becomes a rule: an artifact lies in its session's project, and
  goes with the session only where the project is the session's own. The scopes an artifact may
  lie in change from session, owner's and global to a project's alone.
- Issue [453](453-artifact-publish-takes-eight-parameters.md) stays: `publish` takes the
  channel's view, as `AttachmentManager::create` does, and reads session, channel and the project
  from it.
- Issue [340](340-teams-as-owners.md), the memo: a project in a team's scope gives a team the name
  space of its own the memo asks for; who owns an artifact stays a person.

## Plan

Written on 2026-10-28 from the code as it stands, as the template for the work: one commit per
step, each reviewed before the next. Line numbers are those of that day. Where the plan departs
from the concept above, it says so under *Decisions of the plan*.

### Decisions of the plan

- **The `ProjectManager` is made by the program, beside the `SessionManager`, and holds it.**
  Deleting a project deletes its sessions, and only the `SessionManager` deletes a session - it
  stops what runs (`stop_running`, `leave_actors`). The `SessionManager` is no part of the
  `RuntimeManager`: the programs make it from the built runtime (`caelumora-app/src/main.rs:83`,
  `caelumora-tui/src/main.rs:575`, `caelumora-rest-server/src/routes/mod.rs:306`, the `World`),
  and `ScopedEntities` is fixed in `RuntimeBuilder::build` before it exists. So the
  `ProjectManager` is made the same way, `ProjectManager::new(sessions: Arc<SessionManager>)`,
  and reaches the runtime through it; it is no `ScopeMember`, and sessions are none either: the
  manager lists the sessions of a project by their own field (`Filter::in_project`) and deletes
  each through the `SessionManager`. Weighed against it: the `SessionManager` as a `ScopeMember`
  of project scopes, which would need a member registered after the build and a cycle of
  references; and the `ProjectManager` in the builder without the sessions, which could not
  delete a project.
- **The `SessionManager` never calls the `ProjectManager`** - it could not, the other way round.
  What a project is - its name, description, members, access list, and where it lies - is
  changed by a `ProjectController`, one per project and instance in the `ControllerManager`, as
  a workspace is changed by its `WorkspaceController`: the one writer of the document, which a
  running system will read live - its description, in a prompt one day. The `SessionManager`
  reaches it there (`ControllerManager::project_controller`) to share a project with a session's
  participants and to take the share back. Making a session's own project and taking it along
  are functions of `caelumora_runtime::project` over the stores, as `group::keep_participants`
  is a function over the groups' store: making and deleting are never a controller's. The
  `ProjectManager` keeps the lifecycle alone: `create`, `delete`, `restore`, `purge`.
- **A session's own project is found by a fixed name in the session's scope**, `Project::OWN`
  (`Name::fixed`), as the participants' group is named `participants`; its id is its own. An id
  derived from the session's, as `GroupId::of_session` is, would make its members' group collide
  with the session's participants' group once the project is lifted.
- **A project lies in the global scope, its owner's or a session's; not yet in a group's.**
  `Scope::GroupScope` is "not yet a place anything is looked up from" (`visible_from` answers
  `false`), and the REST API has no group scope on the wire (`ScopeKind::of` reports it as
  `global`). A project in a team's scope is the later step the memo [340](340-teams-as-owners.md)
  describes, together with what the group scope is for; its members are a list meanwhile.
- **What lies in a project's scope is used nowhere yet** (`Scope::admits` answers `false` for a
  project's scope): nothing but artifacts lies there, and they are not used at a place; whether
  a place is in the project, the place's session says, not the scope. `true` would let a
  workspace of a project be attached to every session once one can lie there. Skills,
  personalities and workspaces in projects come with *Lookups from a place*, which gives
  `admits` the chain of the place from its caller.
- **`ScopeNotEmpty` becomes an error of `scoped`**, raised by whoever holds the scope. Today it
  is `SessionError::ScopeNotEmpty`, raised inside `ScopedEntities::delete_lying_in`; a project
  holds a scope too, and adds its sessions to what is left. Step 0.

### Step 0: restructuring, without a change of behaviour

Two commits, each reviewable on its own.

**0a. What is left in a scope is the holder's to refuse.** `caelumora-runtime/src/scoped.rs`:

```rust
/// What a holder's scope still holds after a deletion: by kind, how many
/// the deleter may not delete.
#[derive(Debug, thiserror::Error)]
#[error("…")]  // the text of SessionError::ScopeNotEmpty today, with shown_holder
pub struct ScopeNotEmpty { pub holder: Scope, pub left: Vec<(EntityKind, usize)> }

impl ScopedEntities {
    /// Deletes what lies in `holder` on behalf of `ctx`, as far as they may; answers what is
    /// left, by kind - empty if nothing.
    pub(crate) async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> StoreResult<Vec<(EntityKind, usize)>>;
}
```

`SessionError::ScopeNotEmpty { holder, left }` becomes `SessionError::ScopeNotEmpty(#[from]
ScopeNotEmpty)`; `SessionManager::delete` (`session/manager.rs:636`) and
`SessionLoop::delete_channel` (`session/channels.rs:405`) raise it from what is left.
`shown_holder` (`session/mod.rs:104`) moves to `scoped.rs` with the error. REST:
`error.rs:387` maps the new variant alike (`409 scope_not_empty`); its test at `error.rs:708`.
Docs: `CHANNELS.md:20-28`, `SECURITY.md:217-220`, `scoped.rs` module doc name the new type.

**0b. Issue 453: `publish` takes the channel's view.** `caelumora-artifacts/src/manager.rs:190`:

```rust
/// Publishes `content` … as a version of the artifact `name` of the session `channel` is in,
/// published in `channel` by its viewer, for the user they act for …
pub async fn publish(&self, channel: &ChannelView, (size, content): (u64, ByteStream), classification: &Classification, name: &str, description: Option<&str>) -> ArtifactResult<(Artifact, u32)>;
```

The context is `channel.views().context()`, session and channel `channel.session().id()` and
`channel.id()`; the `expect(clippy::too_many_arguments)` goes. `component.rs:165-168` reaches the
view as `attachment_tool` does (`turn.situation().channel()`, `ExecutionFailed` if the turn is in
no channel). Tests: the helper `publish` (`tests/artifacts.rs:345`) reaches the view through
`world.manager.views(Viewer::new(ctx)).session(..).channel(..)`;
`a_storage_of_content_takes_nothing_above_its_destination` (`:191`), which publishes on a bare
runtime without a session, publishes in a session of the `World` instead. The security suite's
recorded checks gain the view's questions (`GetSession`, `channel_reading`) where they were not
asked before: the assertions of `:438` and `:531` are extended, not loosened.
`PERMISSION-CHECKS.md:214` names the view's reach. Issue 453 is closed with this commit.

### Step 1: the model (`caelumora-model`)

**`src/project.rs`, new.** Module doc: "A project (issue 467): the bracket above a session that
holds what outlasts one piece of work - its artifacts - and the name space they are found in."

```rust
define_id!(ProjectId);

/// A change of a project other than making it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProjectUpdateEvent {
    NameChanged(String),
    DescriptionChanged(Option<String>),
    /// Its members changed; the group of its members follows them.
    MembersChanged,
    AccessChanged,
    /// Lifted into another scope, under a name (`ScopedName`).
    Lifted(ScopedName),
}

/// What a query of projects may name: where it lies, its owner, its members.
pub enum ProjectField { Scope, Owner, Members, Member }   // paths as GroupField's
impl ScopedField for ProjectField { const SCOPE: Self = ProjectField::Scope; }

pub struct Project {
    pub id: ProjectId,
    /// What it is found by and where it lies: a session's scope for a session's own, its
    /// owner's or the global one for a shared one.
    #[serde(flatten)] pub name: ScopedName,
    pub description: Option<Description>,
    /// The users who work on it; empty for a session's own.
    pub members: Vec<UserId>,
    /// Owned by a user; its list names the group of its members for reading, and the
    /// participants of every session that lies in it.
    pub security_attributes: SecurityAttributes,
    pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc>,
    pub deleted: Option<Removal>,
}

impl Project {
    /// The fixed name of a session's own project in the session's scope.
    pub const OWN: &'static str = "own";
    /// A shared project of `name`, owned by `owner`, read by the group of its members.
    pub fn new(name: ScopedName, owner: UserId) -> Self;
    /// The own project of `session`: named `OWN` in its scope, owned by its owner, read by its
    /// participants (`GroupId::of_session`) besides.
    pub fn own_of(session_id: SessionId, owner: UserId) -> Self;
    /// Whether it is a session's own: it lies in a session's scope.
    pub fn is_own(&self) -> bool;
    pub fn owner(&self) -> Option<UserId>;
}
impl Filter<ProjectField> { pub fn with_member(user: UserId) -> Self; }
```

`impl Stored`: `KIND = "project"`, `Within = NoEntity`, `Field = ProjectField`, `unique_name`,
`validate`: the owner is a user; it lies in the global scope, its owner's or a session's - in a
session's named `OWN` and without members, since whoever reads the session reads it; never a
channel's, a group's or a project's; members are users. `impl Updatable` (`Lifted` sets `name`),
`impl Secured`, `impl Scoped`. Tests as `workspace.rs:210-273` has them.

**`src/scope.rs`.** `Scope::ProjectScope { project_id: ProjectId }` with its doc: "A project's:
what lies in it goes with the project. It contains nothing but itself: a session in a project is
told by the session, not by its scope." Arms: `contains` (itself alone), `admits` (`false`, see
the decisions), `key` (`"project:{id}"`), `depth` (project 2, session 3, channel 4),
`From<&Scope> for FieldValue` (`tagged("ProjectScope", tagged("project_id", ..))`),
`validate_users_own` (`Err`: "a {kind} lies in no project's scope yet"). `holders`,
`visible_from`, `lying_within`, `seen_from` need no arm. Tests: the scope lists at `:364`,
`:426-431`, `:453`, and `value/name.rs:166`.

**`src/session.rs`.** `pub project: ProjectId` after `workspace`, documented: "The project its
results go to (issue 467): its own, lying in its scope, or a shared one it lies in." No serde
default: stored sessions are deleted. `SessionUpdateEvent::ProjectChanged(ProjectId)` and its
`apply` arm. `SessionField::Project` (`["project"]`) and `Filter::in_project(project: ProjectId)`.
`SessionTemplate::build` (`template.rs:332`) takes `project: ProjectId`. The literals: the 11 of
the model (`privacy.rs:167`, `channel.rs:520`, `template.rs:340`, `session.rs:249/270/285`,
`store/access.rs:230`, `store/mod.rs:710/725`, `store/suite.rs:141`) and the 13 outside it
(`caelumora-runtime`: `session/testing.rs:92/137`, `controller/session.rs:209`,
`workspace/testing.rs:78`, `testing/mod.rs:425`; `caelumora-agents`:
`components/agent_memory.rs:384`, `prompt/channel_language_prompt_provider.rs:193`,
`…participants_prompt_provider.rs:266`,
`…briefing_prompt_provider.rs:210`; `caelumora-store-json/src/lib.rs:740`).

**`src/group.rs`.** `GroupId::of_project(project: ProjectId) -> Self { Self(project.0) }`;
`Group::of_project(project: &Project)`, named `participants` in the project's scope, owned by the
project's owner, its members the project's; `is_participants` and `validate` take the project's
scope (`ProjectScope { project_id } if self.id == GroupId::of_project(*project_id)`); the module
doc names three kinds of group. `Filter::teams` is right as it is.

**`src/security/access.rs`.** `may_read_project` (`get`) and `may_change_project` (`update`) on
`AccessQuestions`, with the blanket impl. No rule changes: the verbs of a kind are generic
(`ownership.rs:102-126`).

**`src/store/mod.rs`.** `EntityStores::projects` and `open`. **`src/store/suite.rs`**: a fixture
`project(owner)`, the `lifecycle!`, `found_by_name!` and `unique_by_name!` entries, and that a
project of nobody's, or in a channel's scope, or a session's own with members, is refused with
`Validation`. The backends run the suite unchanged; `caelumora-store-postgres` makes a table per
kind in code (`migrations/0001_kinds.sql`), nothing to add.

**`src/snapshot.rs`.** `ProjectSnapshot { id, name, scope, description, members: Vec<(UserId,
Option<String>)>, can_change, access: Vec<AccessSnapshot> }`, as `GroupSnapshot` and
`WorkspaceSnapshot` are; `SessionSnapshot::project: ProjectId`.

**Doc comments:** `security/action.rs:25-44` (what reading a project covers), `event.rs:12-31`.
**Other crates' matches over `Scope`** (compile errors or silent misfiling, step 1 fixes them
all): `caelumora-runtime/src/agent/manager.rs:120` (`ProjectScope` refused: "an agent lies in no
project's scope yet"), `caelumora-skills/src/skill.rs:125-133` (alike), `session/mod.rs:104`
(after 0a in `scoped.rs`: "project"), `view/mod.rs:557` (`group_name`: "the members of project
…"), `caelumora-rest-api/src/scope.rs:29-48` and `access.rs:53-57` (step 4),
`caelumora-artifacts/src/manager.rs:340` (step 3), `workspace/manager.rs:99` (needs no arm).

### Step 2: the runtime (`caelumora-runtime`)

**`src/project/mod.rs`, new.** Module doc: "Projects (issue 467): what a session does with its
project, the errors, and the manager." Exports `ProjectManager`.

```rust
pub enum ProjectError {
    NotFound(ProjectId),
    OwnerNotAUser,
    /// Made, lifted into or deleted where a project cannot be: a session's own on its own, a
    /// scope it cannot lie in.
    InvalidScope,
    UnknownMember(UserId),
    ScopeNotEmpty(#[from] ScopeNotEmpty),
    Invalid(#[from] ValueError),
    Store(#[from] StoreError),
    /// A session of the project that could not be deleted, restored or purged.
    Session(#[source] anyhow::Error),
}
pub type ProjectResult<T> = Result<T, ProjectError>;

/// The own project of session `session_id`, if it still lies in its scope: found by `OWN` in
/// the runtime's name.
pub(crate) async fn own_of(projects: &dyn EntityStore<Project>, session_id: SessionId) -> StoreResult<Option<Project>>;
/// Makes `project` with the group of its members, in the runtime's name, and announces it.
pub(crate) async fn establish(stores: &EntityStores, events: &EventBus, project: &Project) -> StoreResult<()>;
/// Marks, restores and purges a session's own project with its group, in the runtime's name,
/// as `group::delete_participants` does with a group: what lies in its scope is the caller's
/// to delete before, through `ScopedEntities`.
pub(crate) async fn delete_own(stores: &EntityStores, groups: &GroupManager, events: &EventBus, session_id: SessionId) -> StoreResult<()>;
pub(crate) async fn restore_own(…, session_id, since) -> StoreResult<()>;
pub(crate) async fn purge_own(…, session_id) -> StoreResult<()>;
```

**`src/controller/project.rs`, new.** The one writer of a project's document, as
`controller/workspace.rs` is of a workspace's: a `LiveDocument<Project>`, made and held by the
`ControllerManager` once per project and instance (`project_controller(id)`, beside
`workspace_controller`), which takes every announced change of a project into it
(`process_global_event`).

```rust
impl ProjectController {
    pub fn id(&self) -> ProjectId;
    /// The document as it is now.
    pub fn project(&self) -> Arc<Project>;
    /// (`UpdateProject`, its owner's, asked by the store as the change is stored.)
    pub async fn update_name(&self, ctx: &SecurityContext, name: &str) -> ProjectResult<()>;
    pub async fn update_description(&self, ctx, description: Option<&str>) -> ProjectResult<()>;
    /// Sets who works on it - each a user that exists (`UnknownMember`); the group of its
    /// members follows, kept after the change is stored, as a channel's participants' is.
    pub async fn update_members(&self, ctx, members: Vec<UserId>) -> ProjectResult<()>;
    /// Gives it the access list `acl` (`require_nameable`, as a workspace's).
    pub async fn update_access(&self, ctx, acl: Vec<AclEntry>) -> ProjectResult<()>;
    /// Lets `subject` read it, an entry added unless there (`WorkspaceController::share`);
    /// `unshare` takes the entry away. Asked as `UpdateProject`, in the runtime's name where a
    /// session shares the project it lies in: the session's act was asked.
    pub async fn share(&self, ctx, subject: Subject, grant: Grant) -> ProjectResult<()>;
    pub async fn unshare(&self, ctx, subject: Subject) -> ProjectResult<()>;
    /// Lifts it into `scope` - the user's own or the global one - under `name`
    /// (`Lifted { name, scope }`): a session's own becomes a shared project with all that lies
    /// in it, the session included; the store refuses it under `OWN` (`Validation`), and
    /// `NameTaken` where the name is taken.
    pub async fn lift(&self, ctx, name: &str, scope: Scope) -> ProjectResult<()>;
}
```

**`src/project/manager.rs`.** Fields: `sessions: Arc<SessionManager>`, and from its runtime
`projects`, `stores`, `groups: Arc<GroupManager>`, `events`, `access_policy`, `scoped:
Arc<ScopedEntities>`, `controllers`, `writing: Mutex<()>`.

```rust
impl ProjectManager {
    pub fn new(sessions: Arc<SessionManager>) -> Self;
    /// Makes a shared project of `name` in `scope` - the global one or the user's own - for
    /// the user `ctx` acts for (`CreateProject`); `InvalidScope` for a session's.
    pub async fn create(&self, ctx: &SecurityContext, name: &str, scope: Scope, description: Option<&str>) -> ProjectResult<Project>;
    /// Deletes project `id` (`DeleteProject`): marked first, then its sessions in the caller's
    /// name through the `SessionManager`, then what lies in its scope through `ScopedEntities`,
    /// then its group; `ScopeNotEmpty` with the sessions and the rest left, the mark taken away
    /// again. `InvalidScope` for a session's own.
    pub async fn delete(&self, ctx, id) -> ProjectResult<bool>;
    /// Restores it (`Restore`): the project, its group, then its sessions deleted with it and
    /// what lies in its scope, as far as the caller may.
    pub async fn restore(&self, ctx, id) -> ProjectResult<Project>;
    /// Purges it (`Purge`): its deleted sessions, what lies in its scope, its group, itself.
    pub async fn purge(&self, ctx, id) -> ProjectResult<bool>;
}
```

The order of `delete`, `restore` and `purge` follows `SessionManager`'s (`session/manager.rs:604`,
`675`, `719`), the sessions in the place its channels have.

**`src/session/manager.rs`.**
- `create_from_template(…, project: Option<ProjectId>)` (`:204`): with `Some`, the project is
  reached in the caller's name (`projects.get`, `GetProject`), `InvalidScope` if it is a session's
  own; `template.build(…, project_id)` with the chosen id, or the id of the own project made
  before.
- `establish` (`:336`): after the session document, `project::establish` of the own project
  (`Project::own_of`), beside `keep_participants`; the roll-back purges it with the groups
  (`:355-368`). After `complete`, a shared project is shared with `GroupId::of_session` through
  its controller (`project_controller(id).share`, where the workspace is shared, `:255`).
- `delete` (`:604`): after the session's scope, `scoped.delete_lying_in(context,
  &ProjectScope { own })` - the artifacts of the own project, in the deleter's name, as today in
  the session's scope -, what is left added to `ScopeNotEmpty`; then `project::delete_own`
  (which marks it and deletes its group) beside `delete_participants`. `restore` and `purge`
  alike, with `restore_own` and `purge_own`.
- `pub fn runtime(&self) -> &Arc<RuntimeManager>` for the `ProjectManager`.

**`src/session/settings.rs`.** As `update_workspace` (`:112`):

```rust
/// Puts the session into project `project` (`UpdateSession`, and the project reached:
/// `GetProject`), or takes it out into its own with `None` - the one in its scope, or a new
/// one if it was lifted. The project it lies in is shared with its participants; the share of
/// the one it leaves is taken away. Nothing moves.
pub async fn update_project(&self, context: &SecurityContext, project: Option<ProjectId>) -> Result<()>;
```

`SessionController::update_project` (`controller/session.rs`, beside `update_workspace:115`)
emits `ProjectChanged`. `SessionError::ProjectOutOfScope(ProjectId)` for a session's own named
as a shared one.

**`src/group/`.** `delete_participants` and `restore_participants` list by `lying_within(holder)`
and need no change for a project's scope; `GroupManager::update/delete/restore/purge` refuse the
project's group through `is_participants`. `group/mod.rs:6` doc.

**`src/view/`.** `view/project.rs`, new: `ProjectView { views, project }` with `of`, `pub fn
views()`, `pub fn id()`, `name`, `scope`, `description`, `members`, `is_own`, `may_change`
(`may_change_project`), `snapshot() -> ProjectSnapshot` (members' names read as `GroupView`
reads them, `access` for whoever may change). `Views::projects()` (the store lists what the
viewer reaches: owner, members by the group, the list), `project(id)`, `deleted_projects()`,
`ViewError::ProjectNotFound(ProjectId)`; `SessionView::project() -> ProjectId` and
`SessionSnapshot::project`. `group_name` (`view/mod.rs:556`) names a project's group.

**`src/runtime/`.** Nothing: the `ProjectManager` is the program's. `error.rs`: `user_message`
judges `ProjectError`.

**Tests.** `testing/mod.rs`: `World::projects: Arc<ProjectManager>`; `session_of` (`:425`) makes
the own project as `establish` does (or goes through `SessionManager::create`, which is the
better fix: a finding to assess); `not_found` knows `ProjectError`; a helper `World::project`.
`session/testing.rs:134,206,212` and `workspace/testing.rs:78` the same. The security suite
(`tests/security_suite.rs`), after the workspace tests (`:1288-1460`):
- `a_project_is_its_owners_alone` (create in own or global scope; another's refused; rename,
  describe, members, access - `UpdateProject`);
- `a_project_is_read_by_its_members_and_whom_its_list_names` (the group follows `set_members`);
- `a_session_is_made_with_a_project_of_its_own_read_by_its_participants`;
- `a_session_is_put_into_a_project_by_who_may_change_it_and_read_the_project` (the share
  comes and goes; a session's own refused as the target; nothing moves);
- `lifting_a_sessions_own_project_makes_it_shared_with_the_session_in_it` (`NameTaken`; under
  `OWN`, `Validation`);
- `deleting_a_project_deletes_its_sessions_and_what_lies_in_it_as_far_as_the_deleter_may`
  (`ScopeNotEmpty` with `(Session::KIND, 1)` for another's session; restore; purge);
- `deleting_a_session_deletes_its_own_project_with_what_lies_in_it`;
- the asked sequences of `deleting_a_session_needs_owning_it` (`:844-850`) and
  `only_its_owner_attaches_a_workspace…` (`:1421`) gain the own project's lines.

### Step 3: the artifacts (`caelumora-artifacts`)

- **`artifact.rs`**: module doc (`:3-5`); `ArtifactUpdateEvent::ScopeChanged` removed; the doc of
  `name` (`:89-91`: "lies in its session's project"); `validate` (`:252`) asks for a project's
  scope and an owner who is a user, no longer `validate_users_own`; `ArtifactSnapshot::can_change`
  doc; tests `:361-387` with a project's scope.
- **`manager.rs`**: module doc (`:10-14`); `InvalidScope` and `keep` (`:328-351`) removed;
  `publish(ctx, channel: Address<Channel>, content, classification, name: &ScopedName,
  description)`: the caller builds the name in its session's project's scope - `artifact_tool`
  from what the turn's session holds (`Session::project`) - and vouches for it as for the
  channel; the manager refuses a name in any other kind of scope (`Invalid`), and the project
  is reached in the caller's name before anything is kept (`projects.get(ctx, ..)`, `GetProject`,
  as a workspace made in a session's scope reaches the session: `NotFound` → `PermissionDenied`
  of the store); the artifact's list gains `Subject::Group(GroupId::of_project(..))` for reading
  beside the channel's (`:221-235`), the version's the channel's alone. The manager takes
  `projects: Arc<dyn EntityStore<Project>>` (`plugin.rs:28`). `ScopeMember` stays as it is.
- **`view.rs`**: `ArtifactProjectViews` on `ProjectView`, `artifacts()`: what lies in
  `ProjectScope`, each `readable_by`, as `ArtifactChannelViews` does; module doc.
- **`access.rs:14-16`** doc. **`api.rs`**: `ArtifactResponse` loses `scope` and `session_id`,
  gains `project_id: ProjectId`; `KeepArtifactRequest` removed; module doc.
- **`routes.rs`**: `PUT /artifacts/{id}/scope` and `keep_artifact` removed with the
  `InvalidScope` arm; `GET /projects/{id}/artifacts` → `project_artifacts`, through
  `views.project(id)` and the extension.
- **`component.rs:115`**: "an artifact of that name in this project is someone else's".
- **`tests/artifacts.rs`**: `:438` (a project's scope; the member group on the artifact, not the
  version; what was published goes with the session - its own project -, nothing is kept);
  `:531`, `:572`, `:593` (the session's own project with its artifacts, `ScopeNotEmpty` as before);
  `:623` (the owner's `access` names the project's group); `:676` becomes "an artifact of a lifted
  project drops its channel once the session is purged"; new: `an_artifact_of_a_shared_project_
  outlives_the_session_it_was_published_in`, `a_member_of_the_project_reads_the_artifact_and_the_
  versions_of_the_channels_they_were_in`, `publishing_into_a_project_needs_reading_it`.
- `caelumora-runtime/src/scoped.rs:3-6, 26-28`, `caelumora-model/src/scope.rs:205-209`,
  `event.rs:30-31` docs.

### Step 4: REST and the web UI

**`caelumora-rest-api`.** `src/project.rs`, new: `ProjectResponse { id, name, scope: ScopeKind,
session_id: Option<SessionId>, description, members: Vec<MemberResponse { id, name }>,
can_change, access: Vec<AccessEntry> }` from `ProjectSnapshot`; `CreateProjectRequest { name,
scope: ScopeKind, description }`, `RenameProjectRequest`, `LiftProjectRequest { name, scope:
ScopeKind }`, `SessionProjectRequest { project_id }`. `src/scope.rs`: `ScopeKind::Project`,
`of` and `placed` answering the project's id in a third place (`GroupResponse::project_id`, for
a project's members' group); the doc at `:1-13`. `src/access.rs`: `AccessKind::Project` (`of`:
a group in a project's scope; `into_acl`: `GroupId::of_project`). `src/session.rs`:
`CreateSessionRequest::project: Option<Uuid>`, `SessionResponse::project_id`,
`SessionSnapshot::project_id`.

**`caelumora-rest-server`.** `src/state.rs`: `project_manager: Arc<ProjectManager>` (made in
`caelumora-app/src/main.rs:83` beside the session manager, `routes::tests::state` at
`routes/mod.rs:306`). `src/routes/projects.rs`, new, after `workspaces.rs` and `groups.rs`:

| Route | Does |
|---|---|
| `GET /projects` (`?deleted`) | the viewer's projects, or the deleted ones |
| `POST /projects` (201) | `ProjectManager::create` |
| `GET`, `PATCH`, `DELETE /projects/{id}` | fetch, rename, delete |
| `PUT /projects/{id}/description`, `…/members`, `…/access` | describe, set members, share |
| `POST /projects/{id}/lift` | lift |
| `POST /projects/{id}/restore`, `…/purge` | restore, purge |
| `PUT`, `DELETE /sessions/{id}/project` | put the session into a project, take it out |

`routes/mod.rs:143` wires them; `routes/sessions.rs:18` passes the project to
`create_from_template`. `src/error.rs`: `impl From<ProjectError>` (`404 project_not_found`,
`400 invalid_scope`, `400 needs_name`, `400 unknown_member`, `409 scope_not_empty`,
`ProjectError::Session` by its cause), `ViewError::ProjectNotFound`, `SessionError::
ProjectOutOfScope` (`400 project_out_of_scope`), the `answer_as!` entry (`:456`), and the tests
of `answer` (`:678`).

**`caelumora-web-ui`.** `types/index.ts`: `ProjectResponse`, `CreateProjectRequest`, `ScopeKind`
with `'project'`, `AccessEntry.kind` with `'project'`, `project_id` on `SessionResponse`,
`SessionSnapshot`, `CreateSessionRequest`, `ArtifactResponse` (`scope`, `session_id` removed).
`api/index.ts`: the project calls, `setSessionProject`, `liftProject`; `keepArtifact` removed.
`page.ts`: `'project'`; `App.tsx:126`: the branch; `scope.ts`: `'project'` → "of one project".
Components: `ProjectPage.tsx` (what it is, its members as `GroupPage` edits them, `AccessList`,
its artifacts, its sessions; rename, describe, delete), `NewProjectDialog.tsx`,
`SessionProjectDialog.tsx` (the session's project: its own, lifted under a name, or a shared one
chosen; "none" takes it out), `NewSessionDialog.tsx` (a project select beside the workspace),
`Sidebar.tsx` (the list "Projects" after workspaces, the session menu entry "Project", reload on
its events), `ArtifactPage.tsx:106,119-122` (the project named instead of the name space, the
buttons to keep removed), `ToolCallView.tsx:666-727` (`useArtifact` without `keep`, "Open"
alone), `AccessList.tsx:14-21` (the label "project members"). Checked with `npm run build` and
`npm run lint`.

### Step 5: the documents

- `docs/PROJECTS.md`, new: what a project is and what it is for beside a session and a channel;
  a session's own project and lifting; a session in a project; who reads what; the lifecycle;
  where the `ProjectManager` is made and why; what is not done yet (group scope, lookups from a
  place, workspaces in projects). The table in `AGENTS.md`.
- `ARCHITECTURE.md` (the concepts, the managers), `CHANNELS.md:12-33` (what goes with a session:
  its own project), `SECURITY.md:119-180` (a project's ownership; the artifact's bullet),
  `:321-347` (three kinds of group), `PERMISSION-CHECKS.md` (the rows of publishing, of the
  project's actions, of putting a session into one), `STORAGE.md:189-215` (where a project and an
  artifact lie), `REST-SERVER.md:99-108`, `WEB-UI.md:156-209`, `TOOLS.md:300-320`, `EVENTS.md`
  (the kinds), `PLUGINS.md:68-70` (the extension of `ProjectView`).
- Issues: 467 and 453 `done`, 338 `rejected` with the reason, each with its history.

Stored data to delete: every session, since each now names its project, and every artifact, since
none lies in a project's scope.

## Done when

- Every session has a project: its own, made with it, or a shared one it was made in or put into;
  taking it out gives it its own again, and nothing moves either way.
- An artifact lies in its session's project; one of a shared project outlives the session, one of
  the session's own goes with it.
- Lifting a session's own project makes it a shared one, with the session and its artifacts.
- A project is deleted, restored and purged with its sessions and artifacts, in the deleter's name.
- The members of a project read its artifacts, and the versions of the channels they took part in.
- `docs/` say what the three brackets are for and where an artifact lies.

## History

- **2026-10-28** — Opened with the maintainer while taking up issue 338 and
  [453](453-artifact-publish-takes-eight-parameters.md): the maintainer saw in the one
  `ArtifactManager` beside the many workspaces the sign that a container for results was missing,
  and named the project. The concept above was worked out in that discussion, with the decisions
  recorded.
- **2026-10-28** — Revised the same day, after the maintainer's corrections: a session has a
  project, not only a scope, and projects live longer than their sessions. As first written, a
  session's own project was its scope with no entity, an artifact could lie in a session's scope,
  a project could not be deleted while a session was in it, and `keep` stayed as a move of one
  artifact into a project. Decided instead: every session has a project of its own, kept as its
  participants' group is; a session lies in a shared project's scope and goes with it; keeping is
  lifting the session's own project, and no artifact is moved by itself. The three questions
  left open - the move, who may publish, members as a list - were decided as recorded above.
- **2026-10-28** — The plan written from the code and agreed with the maintainer, decision by
  decision: the `ProjectManager` made by the program beside the `SessionManager`, which it holds;
  a `ProjectController` as the one writer of a project, proposed by the maintainer since its
  description will be read live; a session's own project found by the name `own`; no group's
  scope yet; nothing used from a project's scope yet. In progress from here, step by step.
- **2026-10-28** — Step 0a done: `ScopeNotEmpty` is an error of `scoped`, raised by what holds
  the scope from what `ScopedEntities::delete_lying_in` answers is left; `SessionError` wraps
  it, its message unchanged. Beyond the plan, at the reviewer's finding and the maintainer's
  word: it answers for itself wherever it travels - `user_message` and the REST API know it, and
  `SessionError`'s answer delegates to it -, so that a holder that forgets to wrap it is not
  answered as a server error, and the second holder, a project, adds no second mapping. Deleting
  a channel with another's agent in its scope is tested now, as deleting a session was.
- **2026-10-28** — Step 0b done, and with it issue 453, not as planned: `ArtifactManager::publish`
  takes the channel's `Address` (`Address<Channel>`), not its view - a view is a layer of access,
  and an interface speaks of coordinates and properties, as the maintainer put it. In step 3 the
  caller builds the `ScopedName` in the project's scope, so that `publish` takes where it lies and
  where it is published as two coordinates.
- **2026-10-28** — Step 1 done: `Project`, `ProjectId` and `ProjectUpdateEvent`
  (`caelumora-model/src/project.rs`), `Scope::ProjectScope`, the members of a project as a third
  kind of group (`GroupId::of_project`, `Group::of_project`, named `members`),
  `EntityStores::projects` and the store suite. A session template, an agent, a skill, a workspace
  and an artifact are refused in a project's scope by their own validation, which nothing is used
  from yet. `own` is a session's own project's name alone, refused anywhere else by the model, so
  that a lifted project is named anew without an error of its own (`NeedsName` dropped from step
  2). `Group::is_participants` became `Group::is_team`, and `GroupError::Participants` became
  `NotATeam` (REST `409 not_a_team`, was `group_of_participants`): the members of a project are
  kept by what they belong to as the participants are.

  Where step 1 ends: it holds what the model is and checks - the kind, its scope, its group, its
  validation and how its changes are applied. What has its first caller in the runtime comes with
  step 2: `Session::project` and what goes with it (`SessionUpdateEvent::ProjectChanged`,
  `Filter::in_project`) - in step 1 every session would have named a project that does not exist -,
  `ProjectSnapshot`, and `may_read_project`/`may_change_project`. The constructors and the update
  event of step 1 are the model's rules, tested in it, and used from step 2 on.

  Different from the plan besides: a project's fields are those of what lies in a scope
  (`ScopeField`), as a workspace's, since nothing asks for its members by a list yet;
  `MembersChanged` carries the members, since whoever reads a project may read them, unlike a
  team's; `Lifted` carries the name as a text and the scope, as every update event carries what the
  model validates as it applies it; `validate_users_own` refuses a project's scope with its message
  of where a user's own thing lies. The doc comments of `event.rs` and `security/action.rs` on what
  reading a project covers, and `STORAGE.md` on where a project lies, come with step 5, where what
  a project is read for is settled.
- **2026-10-28** — Step 2 split in two, to be reviewed apart: 2a, every session has a project of
  its own, and 2b, shared projects - the `ProjectManager`, the `ProjectController`, putting a
  session into a project, the views and `ProjectError`.

  Step 2a done: `Session::project`, and `SessionTemplate::build` builds the session's own project
  with it (`Project::own_of`, found by `Project::own_name`; `Project::holds` gives the scope it
  holds). The `SessionManager` keeps it as it keeps the group of its participants: made with the
  session in the runtime's name (`make_own`); deleted and restored with its mark after the groups,
  all of them or none, each announced (`delete_kept`, `restore_kept`); purged with it (`purge_own`).
  What lies in its scope is deleted and restored in the name of whoever deletes or restores the
  session, together with what lies in the session's: `ScopedEntities` and the `GroupManager`'s
  `…_participants` take several holders now, and count what is left over all of them, which keeps
  the session as `ScopeNotEmpty` of its scope. Marking deleted and purging, which four places did
  alike, is `scoped::purge_for_good`.

  Different from the plan: the functions over the stores of `caelumora_runtime::project` the plan
  agreed are private methods of the `SessionManager`, at the reviewer's finding - it is their one
  caller and holds what they would be handed; `project/` comes with the `ProjectManager` in 2b. The
  plan's `establish` is `make_own`, for a session's own alone. `build` takes no project and returns
  the own project beside the session and its channels, a tuple of three: with the project as a
  parameter it would take eight, and every session is made with its own, also one made in a shared
  project in 2b, which then takes the shared one as the workspace is taken after `build`; the
  maintainer kept the tuple, and CODING-GUIDELINES.md names it as the exception to small return
  values. A session
  whose owner is no user is no longer made, since its project needs a user as owner; the tests that
  made sessions of nobody give them a user. Restoring takes the session's own project back however
  long ago it was deleted, since it goes only with the session, whose mark a restoring that failed
  renews. `World::session_of` makes the own project as `establish` does; that it does not go through
  `SessionManager::create` stays a finding. Nothing reads `Session::project` yet but the tests: the
  views (`SessionView::project`, naming an own project's group by its session) come with 2b, the
  artifacts with step 3. Stored sessions are to be deleted: `project` has no default.
- **2026-10-28** — Step 2b done, after 2a was accepted: shared projects and a session's place in
  one. The model gains `SessionUpdateEvent::ProjectChanged`, `SessionField::Project` and
  `Filter::in_project`. In the runtime: `ProjectError` (`caelumora-runtime/src/project`), judged by
  `user_message`; the `ProjectManager` - `create`, `delete`, `restore`, `purge`, made from the
  `SessionManager` (`SessionManager::runtime`, which replaces the test helper `runtime_of`); the
  `ProjectController` - `update_name`, `update_description`, `update_members`, `update_access`,
  `share`, `unshare`, `lift`, `process_global_event` -, one per project and instance in the
  `ControllerManager` (`project_controller`); `SessionLoop::update_project`, with
  `SessionController::update_project` and `SessionError::ProjectOutOfScope` (REST `400
  project_out_of_scope`). `World::projects` and the security suite's tests of who makes, changes,
  shares, lifts and deletes a project.

  Different from the plan: step 2 has a third part, 2c, for the views - `ProjectView`,
  `Views::projects`, `ProjectSnapshot`, `SessionView::project`, `may_read_project` and
  `may_change_project`, naming an own project's group by its session. `create_from_template` takes
  no project yet: with it it would take eight parameters; a session is made in its own project and
  put into a shared one by `update_project`, which the REST API of step 4 calls, as it is then the
  one caller - whether creating takes the project then is a question for step 4. The programs make
  the `ProjectManager` once the REST API uses it, in step 4. `lift` lifts a session's own alone, and
  into the global scope or its owner's (`InvalidScope` otherwise): it is the act of keeping, not a
  move of shared projects. `update_members` asks `UpdateProject` before whether each member exists,
  as the `GroupManager` asks for a team, and tells who left (`announce_also_to` takes several
  subjects now). Deleting a project that something is left in keeps the project, and what the
  deleter could delete - sessions among it - stays deleted, to be restored by hand, as what lies in
  a session's scope does.

  After the review: `update_project` changes one session's project at a time
  (`SessionLoop::moving`), shares the project the session goes to - or makes its new own, both with
  its group or neither (`make_own`) - before the session is stored there, and takes that back if
  storing fails; the entry of the project it left goes last. `lift` reaches the project and refuses
  it under the lock of its writing, so that who does not read it learns nothing and it is lifted
  once. The controller reads its project as `load`, as the `WorkspaceController` does, not as the
  plan's `project`. `SessionManager::runtime` is the crate's alone.
  Decided with the maintainer: a session that lies in a deleted shared project is not restored
  alone (`SessionError::ProjectDeleted`, REST `409 project_deleted`); it comes back with the
  project.
- **2026-10-28** — Step 2c done, after 2b was accepted: the views. `ProjectView` (`id`,
  `may_change`, `snapshot`), `Views::projects` - the shared ones the store lists for the viewer; a
  session's own is reached through its session -, `Views::project`, `Views::deleted_projects`,
  `ViewError::ProjectNotFound` (REST `404 project_not_found`); `ProjectSnapshot`, and
  `SessionSnapshot::project`, the project its results go to, where the viewer reads it, as its
  workspace; `AccessQuestions::may_change_project`; the group of an own project's members named by
  its session.

  Different from the plan: `ProjectView` has no getters of name, scope, description, members and
  whether it is a session's own, and `AccessQuestions` no `may_read_project`, since nothing asks for
  them yet; `SessionView::project` is not there for the same reason - the snapshot reads the
  session's field. The session's snapshot carries the project's snapshot, as it carries its
  workspace's, not only its id.
  After the review: the views list the shared projects by a condition of the query
  (`Filter::shared_projects`), as teams are listed, and name members as every view names a user
  (`Views::name_of`).
  How a session's own project is named to a client - `own`, as stored, or by its session, as the
  group of its members is - is decided in step 4, with the REST API.
