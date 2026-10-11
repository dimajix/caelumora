# 312 — Not every operation follows the order and the one reach

**Status:** open
**Area:** Identity and access management

## The problem

Issue [304](304-review-of-the-permission-concept.md) set the rules for where permission questions
are asked: an operation reaches its object first - reading it and, where it is classified, being
cleared for it -, then asks its own verb, then the flow, then anything about others; a refusal
before the verb is "not found", after it "forbidden". Reaching is one function, the one the views
use, so that an operation that reaches an object by its id has exactly the rights of one that
navigated to it. The map redone at the end of 304 (`docs/PERMISSION-CHECKS.md`, section
*Findings*) found sites that do not follow these rules yet. They were left for this issue, since
each asks for a decision of its own rather than a mechanical change:

1.  **Creating asks the others before its own verb.** `SessionManager::create_session` asks every
    member's clearance and every agent's personality (`require_admitted`) before `CreateSession`;
    `create_from_template` even makes agents and a workspace before it.
    `SessionLoop::create_channel` asks the members before `CreateChannel` (after `UpdateSession`,
    which the ownership policy decides alike), `restore_channel` before `Restore`, and it asks
    `UpdateSession` before it reads the deleted channel.
2.  **Validation and flow before the own verb in the managers.** `WorkspaceManager::create` and
    `update`, `SkillManager::create` and `update` ask the flow and the clearance of a
    classification before the store asks the verb: who may read but not change a workspace learns
    `ClassificationRefused`, `StorageNotCleared` or `NotCleared` before `PermissionDenied`.
    `GroupManager::set_members` answers `UnknownMember` before the group is read; the group manager
    and `remove_participant` answer structural refusals (`Participants`, `ApproverStays`) before
    the verb. `ArtifactManager::keep` asks its verb first, because its validation looks at the
    artifact's owner; the others leave it to the store.
3.  **The reach differs by path.** The views reach workspaces, skills and artifacts by reading them
    and by the clearance; the managers' operations by id - rename, share, delete - by reading them
    alone. An owner no longer cleared for their workspace does not find it, but can rename, share
    or delete it by its id. `SessionLoop::delete_channel` reaches a channel by `GetChannel` alone,
    `restore_channel` by `ReadDeleted` alone, where the views add the clearance.
4.  **Events without the clearance.** The event bus delivers a channel's events, and those of
    workspaces, skills and artifacts, to whoever may read the entity, without the clearance the
    views ask: a reader of a channel they are not cleared for receives its events but does not
    find the channel (also an open question in CONFIDENTIALITY.md).
5.  **A refused approval is not told.** The approver leaves out the answer of someone who may not
    approve, with a warning; its sender is not told, unlike what the channel does not let in
    (`MessageRejected`).
6.  **Left from the map of 304:** `Reclassified::Note` and `Memory` carry no attributes and are
    decided by authorship alone (F14); a channel's components are built in its owner's name (F18).
7.  **Long doubles.** Attaching a workspace to a session and sharing it asks `UpdateWorkspace` three
    times; an approval answered by a rule asks `ApproveToolCall` up to three times. Each question
    has its reason; whether the path should be shorter is open.

## Why it matters

The order is what keeps a refusal from telling more than it may: a member's clearance asked
before the actor's own verb tells someone who may not create the session whether a colleague is
cleared for a classification. The one reach is what makes "by id" and "by navigation" the same
rights. Where the code differs, the rule in SECURITY.md promises what the code does not hold.

## Considerations

- **The order at creation**: ask the verb explicitly before the members (rule 2 of 304: the
  operation does something first - it looks at others - so it asks before, and the store again),
  and make nothing - agents, a workspace - before it.
- **Validation before the verb**: whether the order is meant for the flow of the actor's own
  classification - it tells nothing about others - or only for questions about others. If the
  latter, the managers are right and the rule needs saying so.
- **Reaching by id**: either the managers' operations reach as the views do (the clearance
  included, "not found" otherwise), or an owner keeps managing what they own without the
  clearance, and the views should show it to them as such.
- **The event bus**: give it the classification of what an event is about, so that it can ask the
  clearance as the views do.
- **The refused approval**: answered to its sender, as `MessageRejected` is, or kept silent since
  a client offers approving only to who may.

## Done when

- Each finding above is fixed, or decided to stay and the reason written into SECURITY.md or the
  map.
- The security suite asserts the order of the operations that change.

## History

- **2026-10-11** — Opened at the end of issue 304, step 5, from the map redone from the code: what
  304 found but did not decide.
