# 461 — Every member of a scope writes the same three loops over what lies in it

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** The managers of agents, workspaces, skills and artifacts, and `Unowned` for the kinds without one, each implement `ScopeMember` with the same three loops - list what lies in the scope, delete, restore or purge each, count or skip what was refused -, differing only in what they announce and what they remove besides.

## The problem

What lies in the scope of a session or channel is deleted, restored and purged with it (issues
[320](320-scoped-deletion-in-the-runtimes-name.md), [178](178-scoped-lookup-by-name.md)), by a
`ScopeMember` per kind (`caelumora-runtime/src/scoped.rs`). It has five implementations:
`AgentManager`, `WorkspaceManager`, `SkillManager` (`caelumora-skills`), `ArtifactManager`
(`caelumora-artifacts`), and `Unowned` for the kinds that have no manager - personalities,
templates, component bundles.

Each writes the same three loops:

- **`delete_lying_in`**: list what lies in the holder (`Filter::lying_within`) in the system's name,
  delete each in the caller's, count what was refused (`deleted_unless_refused`).
- **`restore_lying_in`**: list what was deleted since the holder was (`deleted_since`), restore
  each, skip what was refused or whose name was taken (`restored_unless_refused`).
- **`purge_lying_in`**: list what is deleted, purge each in the system's name.

They differ only in what is particular to the kind: the event announced for each deleted or
restored entity (`publish` with its attributes, or `announce` with the entity), a lock held while
they write (the workspaces'), and what is removed besides on a purge (an agent's state, a
workspace's place, an artifact's content). The helpers that the loops share are already in
`scoped.rs`; the loops themselves are written five times, for every new kind a sixth.

The groups of the participants of a session or channel (`GroupManager::delete_participants`, issue
[348](348-participants-groups-out-of-step-with-their-holder.md)) loop alike, but deliberately
otherwise: all of them or none, in the runtime's name. They stay apart.

## Considerations

- **One implementation over what a kind adds.** `ScopeMember` is implemented once, generically -
  over the store of the kind and what the kind adds for one entity: what to announce once it was
  deleted or restored, what to remove besides once it was purged, under which lock. `Unowned` is
  that with nothing added. A manager names its additions, not the loops.
- **The managers' own operations, per entity.** Each manager deletes, restores and purges one entity
  already, announcing it (`AgentManager::delete`, …); a generic member could call those for each
  entity it lists. But they read the entity again in the caller's name before they ask, which the
  loops have done once for all, and answer in the manager's error type, not the store's; they would
  have to be split into what is asked and what is done.
- **Keep them.** Five copies of three short loops, each next to the kind it is about. Cheaper today,
  but every new kind, a plugin's too, copies them again, and a change to how a scope's members are
  deleted - issue [458](458-deleted-while-it-runs.md) may bring one - is made five times.

## Done when

- The loops over what lies in a scope are written once, and a member of a scope says only what its
  kind adds.

## History

- **2026-10-23** — Opened after the follow-up of [332](332-policy-as-a-predicate.md), which replaced
  `scoped::within` at the seventeen places these loops list what lies in a scope.
