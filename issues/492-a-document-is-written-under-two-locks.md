# 492 — A document is written under two locks that do not know of each other

**Status:** in progress
**Type:** bug
**Priority:** low
**Area:** Persistence
**Summary:** A controller writes a document under its own `LiveDocument::write_lock`, its manager under the `writing` lock of the kind; neither sees the other, and since the store reads and replaces in two steps, a change of the one can overwrite the other's - a deletion among them.

## The problem

Every write of a document goes through `EntityStore::update` or `delete`, which read the stored
document, ask the policy and then replace it (`store/documents.rs`, `CheckedStore`): two steps,
not one. Against two writers overwriting each other there are locks, but two kinds of them:

- A controller - of a session, a channel, an agent, a workspace, a user - writes under the
  `write_lock` of its `LiveDocument` (`caelumora-runtime/src/controller/document.rs`), one per
  document.
- A manager - `WorkspaceManager`, `SkillManager`, `ArtifactManager`, `GroupManager`,
  `ProjectManager` - writes under its `writing: Mutex<()>`, one per kind: when it makes, deletes,
  restores or purges, and when what lies in a scope is deleted with its holder
  (`ScopedEntities::delete_lying_in`, `ScopeMember::writing`, issue
  [461](461-scope-members-repeat-their-loops.md)). `AgentManager` holds none.

Neither lock knows of the other. A workspace renamed by its controller while its manager marks it
deleted - with its session, say - ends either way wrong: the rename replaces the marked document,
and the workspace stands unmarked in the scope of a deleted session; or the mark replaces the
renamed document, and the new name is lost. The same holds for every kind that both a controller
and a manager write.

The lock per kind also holds more than it needs to: `delete_lying_in` keeps it over the whole
loop of a kind - listing, marking, announcing, on a purge removing places and content -, so that
every writer of the kind, in every other session, waits meanwhile. Nothing the lock guards spans
more than one document: names are kept unique by the storage in the step that writes (issue
[250](250-unique-names-per-name-space.md)), and the numbers of an artifact's versions are one
artifact's.

## Why it matters

A lost mark is a document that is there and should not be - in a scope whose holder is gone -,
a lost rename is a change a user made and does not see. Both are rare, since the window is one
store write, and both are silent. The lock per kind, meanwhile, serialises writers that have
nothing to do with each other.

## Considerations

- **The store writes atomically.** Every document carries a version; `replace` succeeds only
  if the version is the one that was read, and answers a conflict otherwise, which the writer
  resolves by reading again and applying its change once more. It holds against every writer of
  the storage, also of another process (issue [122](122-several-instances.md)), as the unique
  names do; the locks against lost updates - the controllers' and the managers' alike - go, and
  with them `ScopeMember::writing`. The cost: all three backends (memory, JSON, Postgres) learn
  the version, and every writer a way to retry.
- **One lock per document, shared by controllers and managers.** A table of locks by kind and
  id, which `LiveDocument` and the managers take alike; `ScopeMember::writing` takes an id, and
  `delete_lying_in` locks each entity for its own write. Closes the gap and frees the unrelated
  writers, within one process only. A step on the way to the first, if the first is too far.
- **Leave it.** The window is small and the kinds few. But every new manager and controller
  pair repeats the gap.

The first is the goal; the second is acceptable as a step towards it.

## Done when

- No two writers of one document can overwrite each other's change, whichever lock or none
  they hold.
- A write of one document does not wait for the writes of other documents of its kind.

## History

- **2026-10-29** — Opened while deciding issue [458](458-deleted-while-it-runs.md), whose first
  plan rested on `ScopeMember::writing` until it showed that the controllers do not take it.
- **2026-10-29** — Decided for the first option, in three steps, together with issue
  [450](450-store-overwrites-a-change-made-meanwhile.md): the revision in the store and the
  storages; the writers that read again and change again where it matters; the locks go. The
  counter is named `Revision`, not version, since an artifact has versions of its own. Step 1
  done: every kind carries a `Revision`, counted up by every write; `DocumentStorage::replace`
  and `remove` are given the revision read and refuse another (`StoreError::Outdated`), in
  memory, in the JSON files under the lock of the kind, which every write now holds, and in
  Postgres by a column named in the `WHERE` (table form 5); `EntityStore::update` takes `&mut T`
  and leaves the revision written; `delete`, `restore` and `purge` read again and write again in
  `CheckedStore`; `Writing::apply` reads again and applies again; the store suite holds every
  backend to it. A write of a user as a provider answers them starts from the record stored.
- **2026-10-29** — Step 2 done: every writer makes its change again when it was refused as
  outdated. A controller changes its document through `Writing::change` alone - a closure applied
  to the document as it stands, read again as stored on a refusal -, which replaced
  `Writing::store` and the changes made by hand on a copy; the groups gone from an access list are
  told apart before the change (`gone_groups`, `prune_groups`). A manager reads the entity again
  (`StoreResultExt::written`): the groups', the skills' and the artifacts' changes, the group of
  the participants, a user's record and profile. A version published meanwhile takes the number,
  and the publication is refused as outdated for the caller to publish again. The planned
  `dyn EntityStore::modify` was not built: a closure fit neither the managers' own errors nor the
  writers that write only where something differs, and the loop in each is three lines around
  what is its own. Two questions the review of this step raised, open for the maintainer: the
  content of a version is kept under a key made of the number read before the write
  (`artifacts/<id>/<number>`), so two publications of one artifact at once keep their content
  under one key and the one refused forgets the other's - held off by the artifact manager's
  lock within one process until step 3, and across instances since issue 03; a key of the
  publication's own, or a number reserved by a write before the content is kept, would end it.
  And a copy never stored carries the first revision, as one stored once does, so a write of
  such a copy passes against a document created meanwhile under its id; the one writer that
  held such a copy, the user's profile controller, now holds none until one is stored, creates
  rather than changes, and takes a creation meanwhile as a refusal.
- **2026-10-29** — The content of a version lies under a key of the publication's own
  (`ArtifactVersion::content`, a `ContentId`; `artifacts/<artifact>/<content>`), decided for the
  first of the options above: two publications at once never share a key, the one written second
  is numbered after the first as the document is written (`Artifact::add_version`), and a
  publication refused as outdated is made again instead of handed back. The second question
  stays as it is: the one writer that held a copy never stored is the profile controller.
