# 467 — Results have no place of their own: a project holds what outlasts a session

**Status:** open
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
members and an access list. It lies in a scope - the global one, a group's, its owner's, or a
session's -, where it is found by name; projects in one scope have different names. And it
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
scope or a group's (`UpdateProject`, a change of the project's scope). It is then a shared
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

## Steps

1. **The model.** `Project`, `ProjectId`, `ProjectField`, `Scope::ProjectScope`,
   `Session::project`, `GroupId::of_project` and `Group::of_project`; `Artifact::validate` for
   the project's scope. The store suite covers the new kind.
2. **The runtime.** `ProjectManager` - create, delete, restore, purge, lift, members, share -,
   its members' group, the project scope in `ScopedEntities` with sessions as its members, the
   rules and questions for projects in `AccessQuestions` and the policy, `Views::project`,
   `ProjectView`; the session's own project made with it, the session put into a project and
   taken out, the project shared with the session's participants; the security suite.
3. **The artifacts.** Published into the project's scope and read by the members; `keep` and
   what served it removed; the plugin's extension of `ProjectView` listing the project's
   artifacts, as `ChannelView::artifacts` lists a channel's; the plugin's tests.
4. **REST and the web UI.** `/projects` and `ProjectResponse`, `AccessKind::Project`; a session
   made in a project, put into one and taken out; lifting; the artifact's page naming its project
   instead of its scope, the buttons to keep removed.
5. **The documents.** A document of its own, `docs/PROJECTS.md`, in the table of `AGENTS.md`;
   `ARCHITECTURE.md`, `CHANNELS.md`, `SECURITY.md`, `PERMISSION-CHECKS.md`, `STORAGE.md`,
   `REST-SERVER.md`, `WEB-UI.md` and `TOOLS.md` where they say where an artifact lies.

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
