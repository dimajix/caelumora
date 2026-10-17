# 347 — An access list keeps its entries for groups that were purged

**Status:** done
**Area:** Identity and access management
**Summary:** An access list keeps its entries for a group once the group is purged - a session long over, a team removed -, so lists grow by entries that name nobody and are cleaned up by hand.

## The problem

An access list names a group by its id (`Subject::Group`, issue [286](286-groups-as-subjects.md)):
a team, or the participants of a session or a channel, which have the session's or channel's id.
When the group is purged - with its session or channel, or a team for good - the entries naming it
stay in every list that names it. They grant nobody anything, since nobody is a member of a group
that is not there, but they stay in the list, are shown to its owner without a name
(`Views::access_snapshots` finds no group to call them by) and are removed by hand, if at all.

Attaching a workspace to a session adds such an entry for every session it is shared with
(`SessionSettings::set_workspace`, issue [289](289-attach-compares-access.md)); one made for a
session names its session's group from the start. A workspace used in many sessions therefore
carries an entry for each of them, long after most were purged.

## Why it matters

- Lists that grow by entries naming nothing are what makes access lists unreadable, and what
  nobody cleans up.
- An entry that names nothing is shown to the owner as an unnamed group: they cannot tell what it
  was, nor that it is safe to remove.

## Considerations

- **When it is removed: as the list is written, not as the group is purged** (decided
  2026-10-14). Removing every entry as a group is purged would need to find every list that names
  it - a query over every kind with an access list, of the core and of the plugins, by subject -
  and would change entities of other owners in the system's name. Removing an entry as the entity
  holding it is stored again costs one look per group the list names, made where the list is
  written, and leaves each list to its own writer. An entry that names nothing harms nobody in the
  meantime: it grants nothing.
- **Gone means purged, not deleted.** The group of a session or channel that was deleted goes to
  the trash with it and comes back when it is restored; an entry naming it must stay until then.
  Only a group that is neither there nor in the trash is gone - which a look in the system's name
  has to tell apart from one the writer may not read.
- **Where it lives.** `group::require_nameable` already looks at the groups a list names, where a
  workspace's or an artifact's list is written (`WorkspaceManager::store_access`,
  `ArtifactManager`). Whether pruning goes next to it - one function the writers call -, or into
  every storing of an entity with a list (a rename, a reclassification), and whether the stores or
  the managers do it, is to be decided. Every kind that keeps access lists should prune alike:
  workspaces, artifacts, skills, personalities, sessions and channels (`Join` entries naming a
  team), and the kinds of plugins through what the core offers them.
- **Until it is written**, the list still holds the entry. A view may show it as gone, so that its
  owner sees why it is there; whether it does is part of this issue.

## Done when

- Storing an entity's access list drops the entries naming a group that was purged, and keeps those
  naming a group in the trash.
- Every kind with an access list does so alike, the plugins' included.
- SECURITY.md says so, under *Access lists* or *Groups*.

## History

- **2026-10-14** — Opened while issue [289](289-attach-compares-access.md) was looked at: since
  issue [221](221-adding-session-participants.md) a session may take in anyone whoever may change
  it names, so no access list covers its future members but `Everyone` or the session's own group,
  and comparing them when attaching cannot spare the entry. What makes the lists grow is entries
  for sessions long purged; the maintainer decided to remove them lazily, as a list is written,
  not as the group is purged.
- **2026-10-14** — Done. `group::prune_gone_groups` drops from the lists given the entries naming
  a group that is neither there nor in the trash, looked for in the system's name; one that cannot
  be looked up stays. It runs on every storing of a workspace (`WorkspaceManager::store_access`
  and `update`) and of an artifact, its versions' lists included (`ArtifactManager::update`,
  through `Artifact::access_lists_mut`) - every time, not every n-th: a list naming no group costs
  nothing, and one naming a few costs a lookup each. Sessions and channels were left out on
  purpose: their lists come from their templates, change only by invitations used up, and do not
  grow. The view needed nothing: an entry whose group is not found was already shown to its owner
  without a name, which the web UI shows as "gone". Stored data need not be deleted.
- **2026-10-14** — At the maintainer's request, done with it: telling a group in the trash from
  one that is gone showed that the group of a session's or channel's participants can be left in
  the trash while the session or channel is there - by a deletion that failed half-way and took
  its mark away again, or by a restoring that did not reach its groups. `keep_participants` then
  found no group, tried to make one under the id taken, and failed at every change of the
  participants. It now restores a group it finds in the trash, in the runtime's name, before its
  members follow: it is called only while the session or channel is there.
