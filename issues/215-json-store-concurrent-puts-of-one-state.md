# 215 — Concurrent puts of one state in the JSON store leave a journal, and can lose a change

**Status:** open
**Area:** Persistence

## The problem

The test `store_impl::json::tests::concurrent_writes_of_one_file_all_succeed_and_leave_no_temporary_files`
fails now and then (seen once in a run of the whole workspace on 2026-10-03; 21 runs of it alone or
of the model crate passed). It puts the same `AgentChannelState` sixteen times at once and expects
the state's directory to hold nothing but `<agent>.json`.

Since [193](193-external-state-rewritten-whole.md), `JsonFileStore::store_state` decides by the
sizes it finds on disk whether a put writes a new snapshot or appends a line to the journal
(`<key>.jsonl`). Two puts at once can see different things: one finds no snapshot and writes it,
another finds the snapshot just renamed into place and appends a line - here an empty one. The
journal is then left beside the snapshot, and the test's assertion fails. This is the likely cause,
read from the code; the extra file of the failed run was not looked at.

The test was written for [62](62-json-store-concurrent-writes.md) - two writes must not share a
temporary file - and that still holds. But the race it now shows is more than the test's:

- A put that writes a snapshot deletes the journal afterwards (`delete_if_exists`). A line another
  put appended in between is deleted with it; if that put's state was newer than the snapshot, its
  change is lost.
- A put that appends computes its line from `state.changes()` - the changes since that state was
  last stored - which assumes the file holds what that state was stored as. With two writers of one
  state, it holds the other's.

## Why it matters

The runtime keeps one writer per state (see [CHANNELS.md](../docs/CHANNELS.md#one-writer-per-document)):
an agent's channel state is stored by its one actor, under its lock. So the race should not happen
in production - but nothing in the store says or enforces that, and the test says the opposite: that
concurrent puts of one state are fine.

## Considerations

1. **Say it and test it so.** The store requires one writer per state; the test is narrowed to what
   62 is about - no temporary file of one write is lost to another, none is left - and accepts the
   journal. Cheapest; leaves the store unsafe for a second writer, which is then documented.
2. **Serialize puts of one state in the store.** A lock per path in `JsonFileStore` (as the
   `AgentStates` handles do for an agent's own state) makes size check, write and journal deletion
   one step. Concurrent puts of one state then behave as sequential ones; the journal logic stays as
   it is.
3. **Both:** the lock as a safety net, and the rule stated anyway.

## Done when

- The test passes every time and tests what it says.
- STORAGE.md says whether a state may have more than one writer, and the store behaves accordingly.

## History

- **2026-10-03** — Issue created after the test failed once in a workspace run during issue 214,
  which does not touch the store.
