# 286 — A workspace or artifact cannot be shared with a team, only with people one by one

**Status:** done
**Area:** Identity and access management

## The problem

An access list (issue [23](23-acls-in-security-attributes.md)) names whom it grants something: a
user, a role, or the members of a session or channel. None of these is a team that users make
themselves: a role is the administrator's, in the configuration, and stands for what someone is in
the organisation; the members of a session are who happens to take part in it. To share a workspace
with "the project" - five people, changing over time -, its owner writes five `Actor` entries and
keeps them up to date on every workspace and artifact of the project by hand.

Groups of the directory (LDAP, a user file's groups) are no answer: issue 23 decided they are no
subject, since the directory is not ours - its names cannot be checked when an entry is written,
differ per backend and do not exist under the file backend. They map onto roles in the user
provider.

## Why it matters

- Working on files together is the target of the first release (PLAN.md); a team shares more than
  one workspace and many artifacts, and a list per thing that names each person drifts.
- Who belongs to the team is one fact; written into every list, it is many copies of it, and
  someone who leaves the team keeps reading what was shared with it.

## Considerations

- **A group of our own is an entity**: a `Group` with an id, a name in a scope (issue 250), an
  owner, its members (users; never agents - issue 255), `deleted`. Users make and manage it; it is
  not configuration.
- **A subject of its own**: `Subject::Group(GroupId)`, matched like `MemberOf` - the policy asks
  its `Memberships` whether the principal (or the caller, issue 264) is a member of the group, and
  reads the one group named.
- **Who may see and change a group**: its owner changes it; who may read it - its members, the
  owner's choice - is a question of its own, since the list of members tells who works together.
  Whether being added to a group needs the member's consent, too.
- **Who may name a group in a list**: anyone who may change the list, or only who may read the
  group? Naming a group one cannot read grants its members something without knowing who they are.
- **Groups in groups**: not in the first step; a cycle and the cost of resolving it are what nesting
  brings.
- **`Scope::GroupScope`** exists unfinished (issue 23, the text of 2026-09-13): whether a group is
  also a scope that definitions lie in is to be decided with it, or the variant removed.

## Done when

- A user can make a group, add and remove members, and name it in the access list of a workspace,
  an artifact, a skill; its members read what it is granted, wherever they act, and who leaves the
  group no longer does.
- The security suite covers a member, a former member and a stranger.
- SECURITY.md describes groups beside the other subjects.

## History

- **2026-10-10** — Opened as issue 23 planned it (decision 3): sharing with a team is wanted, a
  group of our own is an entity of its own with ownership and scope, and comes after 23.
- **2026-10-11** — Chosen as the first step of the target picture of issue
  [304](304-review-of-the-permission-concept.md): a `Group` is an entity with `SecurityAttributes`,
  a list of actors (users and agents, no nesting) and a scope; `Subject::Group(id)` replaces
  `Subject::MemberOf`, and every session and channel keeps an internal group of its participants,
  written by its controller, in its session's scope, readable but not changeable through the group
  API. The conditions are recorded there.
- **2026-10-11** — Done, on the branch `issue-286-groups`, as step 1 of issue 304:
  - `caelumora_model::group::Group` with `GroupId`, its kind of store operations
    (`EntityKind::Group`, `create_group` ... `find_group_by_name`, both stores, the store suite),
    `ActionType::{Get, List, Create, Update, Delete}Group` decided by ownership, the events
    `Group{Created, Updated, Deleted, Restored}`. A team holds users, lies in the global scope or
    its owner's, and is read by its members through an entry of its own list naming it.
  - `Subject::Group` replaces `Subject::MemberOf`; `Membership` is gone. `Memberships::members_of`
    reads the one group named, also a deleted one.
  - The participants of every session and channel are a group of their own, with the session's or
    channel's id - a shared key instead of a field `group_id`, so that a workspace made for a
    session and an artifact published in a channel name it without a lookup. The session, its
    channels and what changes their participants write it in the runtime's name, *after* the
    change - not before, as issue 304 planned: the change is the act that is permitted, and a
    group written first would grant a participant who was then refused. It is derived anew each
    time, so that a failed write is corrected by the next.
  - `GroupManager` (create, rename, set the members, delete, restore, purge) for teams, refusing
    the groups of participants (`GroupError::Participants`); `Views::groups`, `group`,
    `deleted_groups`, `GroupView`; REST `/groups` and `/groups/{id}/members`; the access lists of
    the REST API keep `session` and `channel` for the groups of participants and gain `group`.
  - Decided on the way: who names a group in an access list must read it (`require_nameable`, in
    both `set_access`), asked after their own permission to change the list. Nesting stays out.
    `Scope::GroupScope` carries a `GroupId`; whether a team is a scope stays open.
  - The security suite: G1 (a member, a former member, a stranger), G2 (naming a group one cannot
    read), G3 (the group of a channel's participants follows them). `set_access` of workspaces and
    artifacts now asks `UpdateWorkspace` / `UpdateArtifact` before the store does.
  - Found on the way: an access list of a workspace or artifact that the store refused as invalid
    was answered `500`; it is `400 validation_failed` now.
  - Stored data to delete: every session, channel, workspace and artifact - their lists name
    `member_of`, and sessions and channels have no group of their participants.
  - The web UI: a "Teams" list in the sidebar with its trash, `NewGroupDialog`, `GroupPage`, and
    teams offered in `AccessList`. The REST API lists no users, so a member is chosen from the
    participants of one of the user's sessions - only someone they share a session with.
  - Of "Done when": a skill has no access list a user writes (no `set_access`), so naming a team
    in one is not possible yet; the policy would grant it as for a workspace.
