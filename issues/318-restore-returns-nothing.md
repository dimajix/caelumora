# 318 — A restore returns nothing, so every caller reads what it restored again

**Status:** open
**Area:** Persistence

## The problem

The `restore_…` methods of `EntityStore` return `StoreResult<()>`. Every caller needs the restored
entity afterwards, at least for the security attributes of its `…Restored` event, and gets it in a
way of its own:

| Caller | afterwards |
|---|---|
| `WorkspaceManager::restore` | `get_workspace` |
| `SkillManager::restore` | `get_skill` |
| `ArtifactManager::restore` | `get_artifact` |
| `SessionManager::restore_session` | `get_session` |
| `SessionManager::restore_agent` | rebuilds it: `Agent { deleted: None, ..deleted }` |
| `SessionLoop::restore_channel` | rebuilds it: `Channel { deleted: None, ..channel }` |
| `GroupManager::restore` | the attributes of `get_deleted_group`, read before |
| `restore_scoped_in` (`scoped.rs`) | the attributes of the entry of `list_deleted_…` |

Three ways for one thing. The store has the restored entity in hand (`restore_with`,
`restore_file_with`) and drops it.

The read afterwards is not just one read too many:

- It asks `Read`, which the restore did not need: the store's restore asks `ReadDeleted` and
  `Restore` (`StoreAccess::restore`). A policy that grants these two but not `Read` lets the restore
  succeed and the read fail - the caller then reports an error for a restore that happened, and no
  `…Restored` event is published.
- Between the two calls the entity can be deleted again; the same then happens.

The managers are not alike either. `SessionManager::restore_session` and `restore_agent` return
what they restored; `WorkspaceManager`, `SkillManager`, `ArtifactManager` and `GroupManager::restore`
return `()`.

## Why it matters

- A restore and its result are one operation, done as two, with a window and an extra permission
  between them.
- The same step is written three ways, and a new kind picks one of them at random.
- Managers of the same kind of thing answer differently to the same operation.

## Considerations

- **The store returns the restored entity.** `restore_…` becomes `StoreResult<T>`, with the
  deletion mark removed. This reveals nothing new: whoever may restore must already be allowed to
  read the entity as deleted (`ReadDeleted`), and the returned document is the same one without the
  mark. The generic helpers of both stores return the value they wrote; the eleven trait methods
  follow. `create_…` and `update_…` keep returning `()`: their caller holds the value already, the
  caller of a restore does not.
- **The managers return it as well.** `WorkspaceManager::restore`, `SkillManager::restore`,
  `ArtifactManager::restore` and `GroupManager::restore` return the restored entity, as
  `SessionManager::restore_session` and `restore_agent` do already. `SessionLoop::restore_channel`
  keeps returning its `ChannelLoop`: a restored channel is started again, and the running loop is
  what its caller needs - a deliberate difference.
- **What stays.** The `get_deleted_…` reads before a restore in `restore_session`,
  `restore_agent`, `restore_channel` and `GroupManager::restore` remain; they are needed for their
  own reasons (when it was deleted, the session it belongs to, the participants' admission, whether
  a group is the participants of a session). The REST handlers keep reading through the views
  (`owned`, `readable`, `channel_view`): their responses are snapshots with details the entity does
  not hold.
- **Leave it.** The second read costs little. But the failure after a successful restore is wrong,
  and the three ways are the kind of repetition AGENTS.md asks to fold into one.

The first two together are the answer.

## Done when

- Every `restore_…` of `EntityStore` returns the restored entity, in both stores.
- No caller reads or rebuilds an entity it has just restored.
- `WorkspaceManager`, `SkillManager`, `ArtifactManager` and `GroupManager` return the restored
  entity from `restore`, as the `SessionManager` does.

## History

- **2026-10-11** — Opened at the maintainer's request, after checking every caller of a store's
  `restore_…`: all of them need the entity afterwards, and four read it again under `Read`.
- **2026-10-11** — [314](314-session-manager-handles-agents.md) moved
  `SessionManager::restore_agent` to `AgentManager::restore`, which still rebuilds the agent and
  returns it; the table above names it by its old place.
