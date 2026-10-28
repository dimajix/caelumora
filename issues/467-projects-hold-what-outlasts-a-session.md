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
| **Project** | what outlasts one piece of work: its results, and the name space they are found in | the artifacts; later what else a subject shares - skills, personalities, a workspace |
| **Session** | one piece of work: who takes part, how private it is, its channels, the room it is done in | its workspace, local or from elsewhere; its agents; what is made for it alone |
| **Channel** | one conversation: its classification, its history, who hears what | its messages and notes; who reads a version of an artifact |

A workspace and a project are both places things lie, and stay apart: the workspace is the room the
work is done in, bound to a session as a rule and shared by several at times; the project is where
the results go.

### The project

A project is a stored entity of the core, `Project`, owned by a user, with a name, a description,
members and an access list. It lies in a scope - the global one, a group's or its owner's -, as a
workspace does, where it is found by name; projects in one scope have different names. And it
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
with the project, kept up to date as members change, deleted and restored with it. The group is a
subject an access list can name without naming each member: an artifact published into the
project is read by the group. Members read the project, make sessions in it and publish into it.
The access list, besides, shares the project with whom the owner names - a team that sees it and
its artifacts.

**Where a project lies.** The global scope, a group's or its owner's - as `validate_users_own`
allows for a workspace, less the session's. A project is not a `ScopeMember`: nothing deletes a
user's scope, a group's or the global one. Sub-projects are not done: a project lies in no other
project's scope.

### The session's project scope

Every session has a **project scope**, the scope its results go to: the scope of its project, if it
is in one, and its own scope otherwise - the session-local project, which is nothing but the
session's scope. `Session::project: Option<ProjectId>` names the project, `Session::project_scope()`
derives the scope, in the model, so that nothing else computes it.

A session is given its project when it is made, and whoever may change the session
(`UpdateSession`) and read the project (`GetProject`) puts it into another project, or into none.
When a session is put into a project, the project is shared with the group of the session's
participants for reading, as a workspace attached to a session is shared with them
(`SessionManager`, issue [253](253-workspace.md)): so they may publish into it.

**Nothing moves.** Changing a session's project moves nothing: what was published into the
session's own scope stays there, as long as the session exists, and what was published into the
former project stays in it. Only new publications go to the new project scope. The agents of the
session keep the personalities they were made with, its workspace stays attached; what a session
finds by name from its place changes with the next lookup alone (see *Lookups from a place*).

### Artifacts

An artifact is published into the project scope of its session, found by name there: the
artifact of that name gains a version, or is made with its first, as today within the session -
one not the caller's to add to is refused as such (`WriteArtifact`). The project is the name
space: a `report.csv` of one project and one of another are two artifacts. So the question of
issue 338, how a version is added to another's artifact once names are per owner, does not arise.

Who reads it: the artifact is read by the participants of the channel it was published in, as
today (issues [23](23-acls-in-security-attributes.md) and
[316](316-store-checks-artifact-without-versions.md)), and by the members of the project; a
version is read by the channel's participants alone, so that a draft published where few are
reaches nobody else - a member who took no part in that channel sees that the artifact is there,
and the versions published where they were. An artifact is made
in a project's scope only by who may read the project, as a workspace is made in a session's scope
only by who may read the session.

**Lifetime, deliberately.** An artifact in a project lives as long as the project: deleting the
session it was published in leaves it alone. An artifact published in a session without a project
lies in the session's scope and goes with the session - deleted, restored and purged with it, as
today, now by design: a session without a project is work done for its own sake, and whoever wants
results kept works in a project. `ArtifactManager` stays a `ScopeMember`, now of project scopes as
well; `Artifact::validate` accepts a session's scope and a project's, no other.

**Keeping one afterwards.** What was published outside a project can still be moved into one: the
`keep` of today (`ArtifactManager::keep`, `PUT /artifacts/{id}/scope`) becomes a move into a
project the caller may publish into - its target a project, no longer the owner's scope or the
global one. A second way beside publishing into a project session, kept because the act is a
natural one in the web UI and the mechanism exists; see *To decide*.

### Lifecycle of a project

A `ProjectManager` owns it, as the other managers own their kinds (`create`, `delete`, `restore`,
`purge`, issue [354](354-names-follow-several-schemes.md)): deleting a project deletes what lies
in its scope in the deleter's name through `ScopedEntities`, as deleting a session does, and
refuses with `ScopeNotEmpty` if another's artifact is left; restoring restores what the caller
may; purging purges all of it, the artifacts' content included. Its members' group goes with it.

A session is not in its project's scope - `Session::project` is a reference, as
`Session::workspace` is -, so sessions are not deleted with a project. A project that a session is
still in cannot be deleted: the session would go on publishing into a scope whose holder is gone.
This differs from a workspace, which a session reads as absent once it is deleted: a session only
reads its workspace through the views, but writes into its project. The deleter is told which
sessions are in it, as `ScopeNotEmpty` tells them what is left.

### Lookups from a place

Three kinds of lookup by name exist, and only one depends on where it is made from:

- Agents and workspaces a session template names are found once, when the session is made, in
  exactly the scope the template says (`template.rs`); the id is stored, and a project change
  touches nothing.
- A bundle reference names its scope itself (`ComponentRegistry::resolve`).
- The catalogue of skills lists what is seen from the channel at every use
  (`Filter::seen_from`, `found_from`, issue [293](293-store-queries.md)), and derives the
  scopes around the place from the scope value alone (`Scope::holders`).

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
- The session-local project is the session's scope itself, no entity. A `Project` entity exists for
  real projects: description, access list, members. No storage is bound to a project.
- Changing a session's project moves no artifact; what lies in the session's scope stays as long as
  the session does.
- A session may change its project; what it finds by name changes with the next lookup, nothing
  stored is looked up again.
- A project has members, with a group of their own.

## To decide

- **The move into a project.** Keep `keep` as a move of one artifact into a project, or drop it and
  let whoever wants an artifact kept publish it again in a project session - one way fewer, a
  version history lost. Recommendation: keep the move; the act exists in the web UI, the mechanism
  in the manager, and only its target changes.
- **Who may publish into a project.** Everyone who may read it - the pattern of the workspace made
  in a session's scope -, or its members alone, which would make sharing a project for reading
  what it says. Recommendation: who may read it, following the pattern; the session's participants
  publish through the share the session brought.
- **Members as a list, or as the scope's owner.** A project in a team's scope could take the team
  as its members, one in a user's scope the user, and have no list of its own - one concept fewer,
  but every shared project would need a team, and a project in the global scope would have no
  members at all. Recommendation: the list, parallel to a session's participants, as decided.

## What it replaces

- Issue [338](338-artifacts-go-with-their-session.md) is superseded: its proposal - an artifact in
  its owner's scope, a version added by id, `keep` removed - is not done. Its bug becomes a rule:
  an artifact lies in its session's project scope, and goes with the session only where the session
  is in no project. The scopes an artifact may lie in change from session, owner's and global to
  session and project.
- Issue [453](453-artifact-publish-takes-eight-parameters.md) stays: `publish` takes the
  channel's view, as `AttachmentManager::create` does, and reads session, channel and the project
  scope from it.
- Issue [340](340-teams-as-owners.md), the memo: a project in a team's scope gives a team the name
  space of its own the memo asks for; who owns an artifact stays a person.

## Steps

1. **The model.** `Project`, `ProjectId`, `ProjectField`, `Scope::ProjectScope`,
   `Session::project` and `Session::project_scope`, `GroupId::of_project` and `Group::of_project`;
   `Artifact::validate` for the new scopes. The store suite covers the new kind.
2. **The runtime.** `ProjectManager` - create, delete, restore, purge, members, share -, its
   members' group, the project scope in `ScopedEntities`, the rules and questions for projects in
   `AccessQuestions` and the policy, `Views::project`, `ProjectView`; the session's project given
   at creation and changed, the project shared with the session's participants; the security suite.
3. **The artifacts.** Published into the project scope and read by the members; `keep` as a move
   into a project; the plugin's extension of `ProjectView` listing the project's artifacts, as
   `ChannelView::artifacts` lists a channel's; the plugin's tests.
4. **REST and the web UI.** `/projects` and `ProjectResponse`; a session made in a project, moved
   into one; the artifact's page naming its project, the keep choosing one.
5. **The documents.** A document of its own, `docs/PROJECTS.md`, in the table of `AGENTS.md`;
   `ARCHITECTURE.md`, `CHANNELS.md`, `SECURITY.md`, `PERMISSION-CHECKS.md`, `STORAGE.md`,
   `REST-SERVER.md`, `WEB-UI.md` and `TOOLS.md` where they say where an artifact lies.

Stored data to delete: artifacts lying in a user's scope or the global one.

## Done when

- A session may be made in a project and moved into or out of one; nothing moves with it.
- An artifact published in a project session lies in the project and outlives the session; one
  published outside a project goes with its session.
- A project is deleted, restored and purged with its artifacts, in the deleter's name, and not
  while a session is in it.
- The members of a project read its artifacts, and the versions of the channels they took part in.
- `docs/` say what the three brackets are for and where an artifact lies.

## History

- **2026-10-28** — Opened with the maintainer while taking up issue 338 and
  [453](453-artifact-publish-takes-eight-parameters.md): the maintainer saw in the one
  `ArtifactManager` beside the many workspaces the sign that a container for results was missing,
  and named the
  project. The concept above was worked out in that discussion, with the decisions recorded.
