# 496 — A session's purge removes its channels' documents before their states and attachments

**Status:** in progress
**Type:** bug
**Priority:** low
**Area:** Persistence
**Summary:** `SessionManager::purge` removes the documents of a session and its channels first and their states and attachments afterwards, so that a purge that fails between leaves states and attachments that nothing finds again.

## The problem

Since issue [492](492-a-document-is-written-under-two-locks.md), what has something behind it is
purged in three steps: its purge begun (`EntityStore::begin_purge`, `Stored::purging`), what lies
behind it removed, the document last. A purge that fails between leaves the entity deleted with
its purge begun, and the next purge finishes it - agents, workspaces and artifacts, alone or with
their scope.

A session's purge does it the other way round (`SessionManager::purge`): the session's document
first, then for every channel its document (`purge_channel_document`), its state
(`delete_channel_state`) and its attachments (`AttachmentManager::purge`). If removing a state or
the attachments fails, the documents are gone already: no purge lists the channel any more, and
its state and the content under `attachments/<channel>` stay for good. A restoring between cannot
happen - the session's lock serialises restoring and purging it -, so only the leak is open.
STORAGE.md names it as the exception to the rule.

## Why it matters

A failed purge is rare, and what it leaves is invisible: a channel's history and its attachments,
which the purge was to remove - for a retention policy, say -, stay on disk or in the database with
nothing that reaches them.

## Considerations

- **Begin, remove behind, then the documents, as everything else.** The session's purge begins
  the purge of the session and of each channel, removes the channels' states and attachments,
  then purges the documents. A failure leaves them deleted with their purge begun, and the next
  purge of the session finds them. Costs: the order in `purge`, `purge_channel_document` split into
  its two steps.
- **Leave it.** The session's lock rules out the restoring; the leak needs a failing store. The
  exception stays named in STORAGE.md.

## Done when

- A session's purge that fails part of the way leaves the session deleted, and the next purge
  removes what is left of it, its channels' states and attachments included.

## History

- **2026-10-29** — Opened from the findings of issue
  [492](492-a-document-is-written-under-two-locks.md), which made every other purge begin before
  what lies behind goes.
- **2026-10-30** — Decided for the first option, in two steps. The scope is wider than the issue
  said: the session's document went first, so a failing step left not only its channels' states and
  attachments but everything after it - its channels, what lay in its scope, its groups, its own
  project - with nothing that finds it. Not every channel needs its purge begun: a channel is
  restored only through its running session, so the session's purge begun keeps its channels too.
  Step 1 done: `SessionManager::purge` goes through `purge_behind`, and what it removed after the
  session's document is its `remove_behind`, each channel's state and attachments before its
  document. `SessionManager::restore` answers a session whose purge is begun as `NotFound` before it
  looks for its project, which would otherwise take its own project, purged already, for a shared
  one deleted. Step 2 is the project's purge, which removes what lies behind it first but begins no
  purge, so that within one process only its lock keeps a restoring from coming between.
