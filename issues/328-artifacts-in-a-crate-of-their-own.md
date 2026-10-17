# 328 — Artifacts are enumerated in the core and cannot leave it

**Status:** done
**Area:** Code structure
**Summary:** Artifacts are enumerated in the core like skills, and need seams skills do not - the channel's view, an event that carries a version, a verb shared with workspaces, a dependency on workspaces.

## The problem

Artifacts (issue 271) are a stored subject like skills, and every place issue
[323](323-kinds-enumerated-in-the-core.md) lists for skills has its counterpart for them: the
entity and `ArtifactField` in `caelumora-model/src/artifact.rs`, `ArtifactSnapshot`, the store's
methods per kind, the four events `ArtifactPublished`, `ArtifactUpdated`, `ArtifactDeleted` and
`ArtifactRestored`, the `artifacts` field of `Scoped`, `RuntimeManager::artifact_manager`,
`Views::artifacts`, `artifact`, `deleted_artifacts`, `may_change_artifact`, `may_read_artifact`
and `may_write_artifact` of `AccessQuestions`, the section `artifacts` of `AppConfig`,
`ToolPresentation::ArtifactPublish`, and the routes, wire types and errors of the REST server.
Issue 323, step 11, foresees an issue for artifacts once skills have shown that the seams hold;
this is that issue, opened ahead with what is known to differ from skills.

## Why it matters

Artifacts try seams that skills do not, and each of these needs a decision that the cuts of issue
323 do not yet make:

- **The channel's view knows them.** `ChannelView::artifacts` (`caelumora-runtime/src/view/
  channel.rs`) lists the artifacts with a version published in the channel. Cut 8 of issue 323
  gives a plugin an extension of `Views` only; the view of a channel would need one as well, or
  the listing moves to the plugin's view, asked with the channel as a filter.
- **An event beyond the lifecycle.** `ArtifactPublished(id, version)` carries the version, and does
  not fit the `Created | Updated | Deleted | Restored` of cut 5 without losing it: publishing a
  version is an update of the artifact, but publishing its first one creates it.
- **A verb shared with another kind.** `WRITE`, the custom verb of `may_write_artifact`, is also the
  workspaces' (`caelumora-model/src/security.rs`). Moved out, the constant belongs to neither
  plugin; it stays in the core as a verb any kind may use, or each kind has its own.
- **It depends on workspaces.** `artifact_tool` reads the file it publishes through the
  `WorkspaceManager`, and the local storage of artifacts writes through `terminal::write_whole`. As
  long as workspaces stay in the core this is a dependency of a plugin on the core. Should
  workspaces become a plugin too, one plugin depends on another, for which there is no rule yet:
  whether that is allowed, and how the `RuntimeBuilder` orders their registration.
- **Its storages are backends.** `ArtifactStorageConfig` is one of the closed enums of issue
  [324](324-backends-enumerated-in-the-core.md); with that issue done, S3
  ([272](272-artifacts-in-s3.md)) becomes a crate that adds a backend to this plugin.

## Considerations

- **When:** after issue 323 is done, as its first follow-up. The decisions above are best made
  with what skills have shown, not before; this file collects them so that they are not found
  again while moving.
- **Open issues on artifacts** - [316](316-store-checks-artifact-without-versions.md),
  [317](317-teams-listed-by-a-scan.md), [319](319-scoped-artifact-events-miss-version-readers.md) -
  are better done before the move, in the code as it is, or they move with it.
- **The crate** is `caelumora-artifacts`, layered by the features of cut 11 of issue 323, as
  `caelumora-skills` is.

## Done when

- `caelumora-artifacts` holds the entity, its manager, its storages, its view, the
  `artifact_tool` kind, its routes, its wire types and its tests; the core compiles and passes its
  tests without it and names artifacts nowhere.
- The decisions above - the channel's view, the publishing event, the shared verb, the dependency on
  workspaces - are recorded here and in the documents they change.
- The web UI works as before against the binary that lists the plugin.

## History

- **2026-10-12** — Opened from the discussion of which subjects besides skills could move into
  crates of their own (issue 323), with what artifacts need beyond what skills do.
- **2026-10-12** — Issue 323 is done, skills moved into `caelumora-skills`; what that settled
  and showed for artifacts. Settled by its steps: the event beyond the lifecycle - publishing the
  first version is `created`, a later one `updated` with `ArtifactUpdateEvent::VersionPublished`
  (step 5) -; the presentation of `artifact_tool`, a `ToolPresentation` of its own (step 6). The
  way in is there: a `Plugin` registers the store of its kind, its manager as a service and scope
  member, its component kinds and seed or configuration sections with the `RuntimeBuilder`
  (step 9); its views are an extension trait on `Views` (`SkillViews`), its questions one on every
  `SecurityManager` (`SkillAccess`), its routes a `RestPlugin` with a state of its own, its errors
  its own with `From<…> for ApiError` (step 10). Shown by skills, and to be expected here: the
  runtime's internals the plugin uses have to be opened (for skills the helpers of `scoped`,
  `ApprovalMode`, `ApiError::bad_request`); the `ViewError` variants of the kind become the
  plugin's errors, which `user_message` then no longer knows (issue
  [336](336-errors-of-a-plugin-shown-as-a-failure.md)); the tests that use the kind as an example
  in the core take another kind, and the plugin's own run in the world of the security suite
  (`World::with`, `World::turn`). Still open here: the channel's view (an extension of the channel's
  view, or a listing of the plugin's views by channel), the shared verb `WRITE`, the dependency on
  workspaces - now issue [333](333-workspaces-woven-into-sessions.md), which recommends moving
  artifacts first -, and how an artifact published in a channel is announced as lying there
  (issue [335](335-channel-events-name-their-session.md)).
- **2026-10-13** — Decided with the maintainer, before the move:
  - **The channel's view:** an extension trait of the plugin on `ChannelView`
    (`ArtifactChannelViews`), as `SkillViews` is one on `Views`. The `ChannelView` opens its views
    and how its viewer reads it (`ChannelView::reading`); whether a viewer reads the channel stays
    the channel's to decide, rather than being worked out again by a listing on `Views` with the
    channel as a filter.
  - **The shared verb:** `WRITE` stays in the core, as a verb any kind may use - it is a name and
    `Grant::Write`, both the core's. The questions on artifacts move into the plugin's
    `ArtifactAccess`.
  - **The dependency on workspaces:** while workspaces are the core's, it is a dependency of a
    plugin on the core and needs no rule; the runtime opens what the plugin uses of the terminal
    (`write_whole`, `WriteFailure`). Whether one plugin may depend on another is left to issue
    [333](333-workspaces-woven-into-sessions.md).
  - **Backends of a plugin's service** - not tried by skills: `Plugin::backends` could register a
    backend, but only the core's services were made of them. The `RuntimeBuilder` keeps the
    backends and the services a backend may ask for, and makes a plugin's from its section
    (`RuntimeBuilder::make_named`); a storage such as S3 (issue [272](272-artifacts-in-s3.md))
    registers itself for `dyn ArtifactStorage` from a crate of its own.
  - **The store's suite:** artifacts are the one kind with parts that have lists of their own,
    queried within an element of a collection in an element of a collection (issues 293, 316). The
    suite takes a kind of its own with such parts, so that the core goes on testing what its stores
    can; what is the artifacts' own is tested in the plugin.
  - Issues 335 and 338 are not part of this one: the web UI asks for a channel's artifacts when it
    shows them, and the `ArtifactManager` stays a member of the scope
    (`RuntimeBuilder::scope_member`) until 338 is decided.
- **2026-10-13** — Step 1: the seams prepared in the core, without a change of behaviour - the
  `ChannelView` opens its `views` and `reading`, `WRITE` is documented as the core's, `write_whole`
  and `WriteFailure` are opened, a terminal's error becomes a `ToolError` by `From`, the
  `RuntimeBuilder` keeps the backends and makes a plugin's (`make_named`), and the store's suite
  asks for parts of a kind of its own (`Folder`).
- **2026-10-13** — Step 2: artifacts moved into `caelumora-artifacts`, layered as
  `caelumora-skills` is: the entity, `ArtifactSnapshot` and the wire types without features; the
  `ArtifactManager` with `ArtifactsConfig` and `ArtifactStorage`, the storage `local`, the views
  (`ArtifactViews` on `Views`, `ArtifactChannelViews` on `ChannelView`), the questions
  (`ArtifactAccess`), `artifact_tool` and the plugin with `runtime`; the routes and the answers of
  `ArtifactError` with `server`. The core names artifacts nowhere in its code. What the move asked
  of the core besides step 1: `Views::subjects` - whom an entry of an access list may name the
  viewer by, which the listing of artifacts asked of the private `teams` -, and opened
  `Views::access_snapshots`, `group::require_nameable`, `scope::validate_users_own`,
  `AccessEntry::of` and `routes::attachment_response`. `Detail::PUBLICATIONS` went: where its
  versions were published is asked of an artifact's view (`snapshot_with_publications`). The world
  of the security suite takes the sections of a plugin (`World::configured`), and an agent's call on
  the workspace is made there (`World::asked_for_a_call`), so that the plugin's tests run it; the
  tests that used artifacts as an example in the core - what a session leaves when it is restored,
  the groups announced with it - take an agent and the groups alone. The REST API is unchanged.
