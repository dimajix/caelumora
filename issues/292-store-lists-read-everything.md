# 292 — A list of what a user may read is made by reading everything

**Status:** done
**Area:** Persistence
**Summary:** A list of what a user may read is made by reading every entity of the kind and asking the policy of each - for artifacts, of which there may be hundreds of thousands, per version too.

## The problem

Every list of the stores is made the same way (`store::access`): everything of the kind is read,
and the policy is asked of each entity whether the caller may read it. With the JSON store that is
a file read per entity, with any store a policy question per entity - and since issue
[23](23-acls-in-security-attributes.md) a question that may read the store again, once per entry
`MemberOf` a session or channel (`StoreMemberships`).

Artifacts make it worst, because there can be hundreds of thousands of them:

- `ArtifactManager::published_in` - what was published in a channel,
  `GET …/channels/{cid}/artifacts` - reads every artifact in the system's name and keeps those
  with a version published there (since issue 271), then asks per version whether the viewer may
  read it (since 23, step 3).
- `ArtifactManager::all_readable` - the user's list, `GET /artifacts` - reads every artifact in the
  system's name and asks per artifact and per version. It cannot ask the store with the user's
  context: the store's check knows the artifact's own list, not those of its versions, and would
  leave out what the user reads only by a version.

Neither is used by the web UI yet - its cards download by the artifact, and it has no list of
artifacts (issue [291](291-share-artifacts-in-the-web-ui.md)) -, but both are open over REST.

Besides, what a viewer may read of artifacts is decided in the `ArtifactManager` (`readable`,
`reached`, `all_readable`, `published_in`), while for everything else the views decide it and the
managers carry out operations that check themselves ([SECURITY.md](../docs/SECURITY.md)).

## Why it matters

- A list that costs a read of every artifact - and more reads for every membership - does not
  serve a deployment that keeps what its agents produce.
- The same pattern holds for every other list (sessions, workspaces, skills, agents); artifacts
  are the first where it hurts.

## Considerations

**Step 1 - the reading into the views, no change in behaviour.** What a viewer may read and list
goes to the views, as for workspaces: `Views::artifacts`, `Views::artifact` - an `ArtifactView` -
and `ChannelView::artifacts` ask per version, by `Artifact::version_attributes`, the one place the
rule lives. Downloading goes through the view (`ArtifactView::content(n)`: the version and the
clearance), and the manager hands out content without asking, as `WorkspaceManager::terminal` does.
The manager keeps the operations: publishing, keeping, describing, sharing, deleting.

**Step 2 - queries instead of scans.** The store answers by an index, the views ask it:

- **By place**: the artifacts with a version published in a channel of a session
  (`list_artifacts_published_in(session, channel)`), from an index over the versions. Then a policy
  question for those few alone.
- **By subject**: the user's list holds what they own and what is shared with them by name - an
  entry `Actor` or `Role` of theirs in the artifact's list or a version's -, from an index over
  owners and subjects. What they reach only as a member of a channel they find through that
  channel (by place); listing it as well would mean enumerating every place they are a member of.
  An index over all subjects, members included, is the larger alternative.
- **Keeping the index**: the memory and JSON stores keep it in memory, built on open and kept with
  every change of a list or a version; a database store ([03](03-postgres-entity-store.md)) keeps
  it as a table or an index (GIN over the subjects).
- **The other lists** - workspaces shared by name, skills, sessions - want the same once they grow;
  the index by subject is to be built so that it can serve every kind, not artifacts alone.

## Done when

- What a viewer may read and list of artifacts is decided in the views; the manager carries out
  operations and hands out content.
- The channel's artifacts and the user's list are answered by store queries that read only what
  they return, and a test shows a list among many artifacts reading only its own.
- STORAGE.md and SECURITY.md describe the indexes and where reading is decided.

## History

- **2026-10-10** — Opened at the maintainer's request, reviewing issue 23: `list_artifacts` may
  return hundreds of thousands of artifacts, and `published_in` and `all_readable` read them all
  in the system's name; reading belongs into the views besides.
- **2026-10-10** — Step 2 - queries instead of scans - went to issue [293](293-store-queries.md),
  opened at the maintainer's request: one declarative condition and a limit for every list,
  rather than methods of their own for artifacts by place and by subject.
- **2026-10-10** — Step 1 done, branch `issue-292-store-lists`, without a change in who may read
  what - the security suite passes as it was: what a viewer may read of artifacts is decided in
  the views (`Views::artifacts`, `Views::artifact` and `ChannelView::artifacts`, by
  `Views::readable_artifact`), which hand out an `ArtifactView` holding the versions the viewer
  may read; its `content` hands out those alone, read by the manager without asking
  (`ArtifactManager::content_of`), as `WorkspaceManager::terminal` does. The manager's
  `readable`, `reached`, `all_readable`, `published_in` and `content` are gone; it carries out
  publishing, keeping, describing, sharing and deleting. One answer changed, aligned with
  workspaces: an artifact whose classification the viewer is not cleared for is not there for
  them (`404 artifact_not_found`), where its download answered `403 not_cleared`
  (`ArtifactError::NotCleared` is gone). The lists still read every artifact; that is 293's.
  Closed with step 1: what remains of *Done when* - the queries - is 293's.
