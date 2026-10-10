# 286 — A workspace or artifact cannot be shared with a team, only with people one by one

**Status:** open
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
