# Projects

A **project** holds what outlasts a piece of work: its results, and the name space they are found in
(issue 467). This document describes what a project is, how a session belongs to one, who reads
what, and how a project lives and goes. How sessions run is in [CHANNELS.md](CHANNELS.md), how
artifacts are published in [TOOLS.md](TOOLS.md#artifacts), the permissions in
[SECURITY.md](SECURITY.md).

## Three brackets

| Bracket | What it is for | What belongs to it |
|---|---|---|
| **Project** | what outlasts one piece of work: its results, and the name space they are found in | the artifacts; its sessions |
| **Session** | one piece of work: who takes part, how private it is, its channels, the room it is done in | its workspace, local or from elsewhere; its agents; what is made for it alone |
| **Channel** | one conversation: its classification, its history, who hears what | its messages and notes; who reads a version of an artifact |

A workspace and a project are both places things lie, and stay apart: the workspace is the room the
work is done in, its storage its own; the project is where the results go, and chooses no storage -
the section `artifacts` does.

## What a project is

A project is a stored entity of the core (`caelumora_model::project::Project`): owned by a user,
with a name, a description, members and an access list. It lies in a scope, where it is found by
name, and it **holds a scope of its own** (`Scope::ProjectScope`): what lies there belongs to the
project - names are unique in it, and it goes with the project as what lies in any scope goes with
its holder (`ScopedEntities`). Artifacts are the first kind that lies there; nothing is used from a
project's scope at a place yet (`Scope::admits` answers `false`), so no agent, skill, workspace or
session template lies there.

A **shared project** lies in the global scope or its owner's. Its **members** are users who work on
it; their group (`GroupId::of_project`, named `members`, in the project's scope) is what its access
list names for reading, and it follows them as the participants' group of a session follows its
participants. Who leaves reads the project no longer, and is told so.

## Every session has a project

`Session::project` names the project a session's results go to, always.

*   **Its own.** A session is made with a project of its own, in the session's scope, named `own` -
    a name no other project may have -, owned by the session's owner and read by its participants
    (`Project::own_of`, built by `SessionTemplate::build`). The session keeps it as it keeps the
    group of its participants: made with it, deleted and restored with its mark, purged with it, in
    the runtime's name; what lies in it goes with the session in the name of whoever deletes or
    restores the session. A session is a user's, so that its project has an owner.
*   **A shared one it lies in.** Whoever may change the session (`UpdateSession`) and reads a shared
    project (`GetProject`) puts the session into it (`SessionLoop::update_project`), or takes it out
    into its own again - the one in its scope, or a new one if that was lifted. The project it lies
    in is shared with the session's participants for reading, in the runtime's name, so that every
    participant publishes into it; the project it leaves is no longer. A new session can be made in
    a shared project: it is made in its own and put there, or not made at all.

**Nothing moves.** What was published into a project stays there; only what comes next goes to the
other. The session keeps its workspace and its agents; what it finds by name does not change, since
nothing is found from a project's scope yet.

**Keeping is lifting.** The owner of a session's own project keeps it by lifting it into their scope
or the global one under a name of its own (`ProjectController::lift`): it is a shared project then,
with the session in it and every artifact published so far. No artifact is kept by itself.

## Who reads what

*   A **project** is read by its owner, its members, whom its list names, and the participants of
    every session that lies in it. Only its owner changes it (`UpdateProject`) - its name,
    description, members, list, where it lies - through its `ProjectController`.
*   An **artifact** published into a project is read by the participants of the channel it was
    published in, and by the members of the project, who read the artifact alone: that it is there,
    and the versions of the channels they took part in. Publishing asks that the publisher reads the
    project, before anything else.
*   The participants' group of a session stays where it is about the session itself - its own list,
    its workspace. The project's members are named where the results are.

## Lifecycle

The `ProjectManager` makes, deletes, restores and purges shared projects
(`caelumora-runtime/src/project`). It is made by the program beside the `SessionManager`, which it
holds, since deleting a project deletes the sessions in it and only the `SessionManager` deletes a
session - it stops what runs. Deleting a project deletes its sessions through the `SessionManager`
and what lies in its scope through `ScopedEntities`, in the deleter's name: a session or anything
else left that they may not delete keeps the project (`ScopeNotEmpty`), and what they could delete
stays deleted. Restoring brings back what went with it, as far as the restorer may; a session in a
deleted project is not restored alone. Purging purges all of it, the artifacts' content included. A
session's own project goes with its session alone.

## Not done yet

*   A project in a team's scope: a group's scope is not yet a place anything is found from (issue
    [340](../issues/340-teams-as-owners.md)); its members are a list meanwhile.
*   Lookups from a place: skills and personalities lying in a project and found from its sessions,
    which needs the chain of scopes around a place given by whoever knows the session.
*   Workspaces in projects, shared by all its sessions.
