# 319 — An artifact deleted or restored with its scope is announced to its own readers only

**Status:** open
**Area:** Identity and access management

## The problem

What is said of an artifact as a whole - its name, its deletion - is heard by whom reads anything
of it: the artifact's own access list and those of all its versions
(`Artifact::reader_attributes`). The `ArtifactManager` announces by these attributes: published,
updated, deleted, restored.

What lies in a session's or channel's scope is deleted and restored with it (`delete_scoped_in`,
`restore_scoped_in` in `caelumora-runtime/src/scoped.rs`, issue 178). There an artifact's
`ArtifactDeleted` and `ArtifactRestored` are published with `Artifact::security_attributes`, the
artifact's own list alone. The event bus asks `GetArtifact` with the attributes it was given
(`event.rs`), so whoever reads the artifact only by a version's list does not hear of it.

That is the common case: an artifact published in a channel reaches the channel's participants
by its version's list (issue [316](316-store-checks-artifact-without-versions.md)). When the
session is deleted or restored, they are not told that its artifacts went or came back, though they
are told when the artifact alone is.

## Why it matters

- The same event of the same artifact reaches different people depending on who deleted it.
- A client that keeps a list of artifacts from the events keeps one that was deleted with its
  session, or misses one restored with it.
- Which attributes an artifact is announced by is decided in two places, and the second one got it
  wrong. Any further place that announces an artifact can do the same.

## Considerations

- **`scoped.rs` uses `reader_attributes`.** It is a one-line fix in two places. The choice stays
  repeated: whoever announces an artifact has to remember the right method.
- **The `ArtifactManager` announces, `scoped.rs` asks it to.** The `ArtifactManager` gets a
  crate-internal operation that deletes or restores an artifact in the runtime's name and announces
  it; `delete_scoped_in` and `restore_scoped_in` call it instead of the store. Announcing an
  artifact is the work of one owner. `purge_scoped_in` takes the `ArtifactManager` already; the
  other two would take it as well, and the same question arises for workspaces and their manager.
- **The event carries the artifact, and the bus picks the attributes.** `GlobalEvent` names ids
  only; it would grow a payload just to choose the receivers. That is the bus deciding what an
  artifact is.

The second is the clean one; the first fixes the symptom at once. Which one is to be decided.

## Done when

- `ArtifactDeleted` and `ArtifactRestored` reach the same people however the artifact was deleted
  or restored: alone, or with the session or channel whose scope it lies in.
- A test shows that a participant who reads an artifact only by its version's list hears of its
  deletion with the session.

## History

- **2026-10-11** — Opened at the maintainer's request, found during issue
  [318](318-restore-returns-nothing.md): `restore_scoped_in` announces a restored artifact with
  its own attributes, the `ArtifactManager` with its readers'; `delete_scoped_in` does the same on
  deleting.
