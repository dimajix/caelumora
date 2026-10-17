# 322 — Whether one is cleared for what one reaches is decided outside the store

**Status:** done
**Area:** Identity and access management
**Summary:** Whether one is cleared for a classified entity is asked by the views, three managers and the event bus each, besides the store, so that what a reader forgets to ask is found, and the deleted are reached by other rules than the live.

## The problem

Issue [304](304-review-of-the-permission-concept.md) made reaching a classified entity one
question in two parts (concept 4 of `docs/PERMISSION-CHECKS.md`): reading it, the
`SecurityManager`'s and asked by the store, and being cleared for its classification, the
`ConfidentialityPolicy`'s. Only the first is asked where entities are read and listed. The second
is added by whoever reads, each on its own:

- the views filter what the store lists - `Views::workspaces`, `skills`, `artifacts`, the
  channels of a session through `channel_reading` - and refuse what they reach by id;
- the workspace, skill and artifact managers reach an entity named by its id with a `reached` of
  their own each (issue [312](312-order-and-reach-not-everywhere.md)), `WorkspaceManager::reached`
  also for `require_attachable`;
- the event bus is given the classification of what an event is about and asks it per party
  (`EventPublisher::publish_classified`, issue 312);
- what was deleted is listed and reached by the store alone (`ReadDeleted`): the trashes show, and
  restoring and purging reach, what the views would not find were it there.

The maintainer's rule is that what a `list_…` returns is the store's to decide, and that a view
does not filter it again (issue 312). Under it, the clearance as the views ask it today is a
second filter beside the store's, written in several places.

## Why it matters

- "Not found" for what one is not cleared for holds only where a reader remembered to ask; a new
  view, manager or operation that reads through the store alone shows it.
- A live entity and a deleted one are reached by different rules: the trash lists a workspace its
  owner is not cleared for, the list of workspaces does not.
- The same question is written in every reader: the views, three managers, the event bus.

## Considerations

- **The store decides.** It is given the `ConfidentialityPolicy` and leaves out of `get_…` and
  `list_…` - also of the deleted - what the context is not cleared for, answering "not found" as
  for what it may not read. The views, the managers' `reached` and the filters fall away. The cost:
  the store, so far ownership alone (concept 1), asks the flow's clearance as well (concept 2), and
  every stored kind that is classified has to say how (`Channel`, `Workspace`, `Skill`, `Artifact`,
  an artifact also per version - issue [316](316-store-checks-artifact-without-versions.md)).
- **The runtime's own reads.** The runtime reads much in the system's name, which is cleared for
  everything (issue 312), so that moving the question into the store changes nothing there.
- **An agent's reach is not a user's.** An agent reads a channel from its place - whole or entry by
  entry by `may_copy_information` -, not by its clearance alone. Whether the store asks the user's
  form only and the agent's stays in the views, or the context says how it reads, is part of the
  decision.
- **The event bus** has no store to ask; it keeps the classification it is given, or asks the
  question the store asks through one shared function.

## History

- **2026-10-11** — Opened from issue 312, where the maintainer decided that lists are the store's
  to decide and the trashes are not filtered again in the views.
- **2026-10-11** — Done, as decided with the maintainer: the store reaches workspaces and skills,
  the clearance included (`Stored::readers_cleared_for`; `MemoryStore::with_policies` and
  `JsonFileStore::new` take a `ConfidentialityPolicy`), for every operation, the deleted alike. The
  views no longer filter the lists of workspaces and skills, `usable_skills` no longer asks the
  clearance, `WorkspaceManager::reached` asks only `WriteWorkspace` besides the store, and
  `SkillManager::reached` is gone. The store's list filter and the event bus ask one function,
  `confidentiality::may_reach`. Channels stay with `view::channel_reading`, since an agent reads
  one it is not cleared for entry by entry from its place, which a context does not carry;
  artifacts follow with issue [316](316-store-checks-artifact-without-versions.md). SECURITY.md,
  STORAGE.md and the map say so.
- **2026-10-11** — Extended: the views read channels and artifacts in the runtime's name, so that
  asking the clearance of them in the store takes nothing from an agent reading another channel
  entry by entry; the reason given above for leaving them out did not hold. `readers_cleared_for`
  became `Stored::classification`, which every classified kind answers - channels, workspaces,
  skills, artifacts -, and the store asks the clearance of all of them. `ArtifactManager::reached`
  is gone; the views keep reaching what they read in the runtime's name themselves:
  `channel_reading` for a channel, the versions' lists for an artifact (issue 316).
- **2026-10-12** — A test of the suite failed after the merge,
  `a_session_goes_with_what_lies_in_it_only_as_far_as_whoever_deletes_it_may`: deleting the
  session's artifacts now asks the clearance for hers (`JoinChannel`), which its expectation did
  not have, and it expected the two artifacts in one order, though the store lists them by their
  random ids. Its expectation takes the clearance and either order; fixed on the branch of issue
  [323](323-kinds-enumerated-in-the-core.md), step 1.
