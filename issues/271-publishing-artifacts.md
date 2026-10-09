# 271 — An agent cannot offer a file for download, and nothing outlives the workspace it lies in

**Status:** in progress
**Area:** Context and knowledge

## The problem

An agent works on the files of the session's workspace (issues [253](253-workspace.md),
[251](251-open-terminal.md)). When it has produced a result - a report, an export, an image - it
can only say in text where the file lies. There is no way for it to offer that file deliberately
for download: a user finds it only by opening the workspace's file browser, and only if they may
use the workspace themselves.

A workspace is a working area. It holds drafts, intermediate files and whatever a command left
behind, and it may change at any time; a session's workspace goes with its session. Should
workspaces ever be cleaned up to reclaim space, everything in them would go - the results with the
rubbish. A result that was handed over should not depend on the working area it was made in.

## Why it matters

What an agent hands over is the point of its work. A user needs to get at it from the
conversation, at the message that offered it, and needs it to stay what it was when it was
offered - also after the agent has gone on changing the file, and after the workspace has been
tidied or deleted.

## Considerations

### How Claude does it

claude.ai separates the two as well:

- Code runs and files are made in an isolated sandbox per conversation - the working area
  ([Create files with Claude](https://support.claude.com/en/articles/12111783-create-files-with-claude)).
  A file meant for the user is handed over deliberately and appears in the chat as a download.
- Artifacts are a thing of their own: versioned, kept with the conversation, published or shared by
  the user, with a lifetime independent of the sandbox
  ([What are artifacts](https://support.claude.com/en/articles/9487310-what-are-artifacts-and-how-do-i-use-them),
  [Publishing and sharing artifacts](https://support.claude.com/en/articles/9547008-publishing-and-sharing-artifacts)).
  Claude Code's artifacts work alike: a page is published from a local file into a store of its
  own, private by default, kept under a link, and published again as a new version.

The common pattern: a mutable working area, and an explicit act that copies a result out of it into
a place with its own, longer lifetime, from where it is downloaded.

### This contradicts an earlier recommendation

[PLAN.md](PLAN.md) (block B1, W1) recommended **one store** for named binary content - "a
generated artifact is a file that a message refers to" - so that workspace files and artifacts
are not two stores, two panels and two permissions. Issue [208](208-generated-artifacts.md)
followed it: an artifact a remote agent or tool generates would be a file written into the
session's workspace, referred to by workspace, path and digest. Its open question was what happens
in a channel without a workspace.

Separate artifact stores would undo that recommendation. The question is whether the difference
has a reason. It has several:

- **Mutable vs. fixed.** A workspace file is changed by agents, people and commands; a published
  result must stay what it was. Referring to a workspace file by digest only detects that it
  changed; it does not keep what was offered (that would need versions, issue
  [256](256-workspace-versions.md)).
- **One label vs. each its own.** A workspace has one classification for all its files
  ([169](169-changing-classified-content.md)). A published result keeps the label it was published
  with, whatever the workspace becomes.
- **Lifetime.** A working area may be tidied or reclaimed; a handed-over result should not go
  with it.
- **No workspace needed.** An external agent's or an MCP tool's file (208) can be kept where no
  workspace is used at all.

### Where artifacts hang: the session or the workspace

**Not the workspace.** Tied to it, an artifact would share exactly the lifetime it is meant to
escape: reclaiming or deleting the workspace would take its artifacts along, and a user workspace
shared by many sessions would gather the results of all of them in one place with one label. And
a channel without a workspace could have no artifacts.

**Their scope decides their lifetime, as for workspaces.** An artifact is published *in* a channel
(at the message that offers it) and *lies in* a scope - the session's, its owner's or the global
one - exactly as a workspace does (`Scope`, `Scoped`, soft deletion):

- in the **session's scope**: kept with the session, deleted, restored and purged with it - but not
  with its workspace;
- in a **user's scope**: kept beyond the session, among that user's artifacts;
- in the **global scope**: kept for everyone it is shared with.

So the three levels proposed are the three scopes Caelumora already has, and need no new concept.

### Options

- **A. Workspaces in a longer-lived scope serve as the artifact store.** No new store: user and
  global workspaces are not reclaimed anyway; a session workspace is the volatile one. Publishing
  copies a file from the session's workspace into a user or global workspace and the message refers
  to it. Keeps PLAN's "one store". But the copy is a mutable file again, under the target
  workspace's one label - a result classified `confidential` cannot go into a workspace that is
  `internal`, so a user needs a publication workspace per classification -, and a channel without
  a workspace still has nowhere to put 208's artifacts.
- **B. Artifacts as a kind of their own, published as fixed copies** (recommended). An `Artifact`
  document - name, media type, size, digest, classification, producer (agent, call), the channel
  and message it was published in, its scope, `SecurityAttributes` - and its content in a
  content-addressed store (by digest: the same file published twice is kept once). `publish_file`
  copies a file of the workspace into it at the moment of publishing; the agent may go on changing
  the file, the artifact stays. 208's producers create artifacts the same way, without a workspace.
  The cost: a second place for binary content, with its own permissions, REST routes and UI - the
  difference to workspaces is deliberate, for the reasons above.
- **C. Keep files in the workspace and make workspaces keep versions (256)**, so that a reference
  by digest can always be resolved. Solves "fixed", not lifetime nor labels, and makes publishing
  depend on W4.

### The tool

`publish_file` (or `publish_artifact`) of `terminal_tool`, beside the file tools:

- parameters: the file's `path`; optionally a `name` to offer it under and a short `description`;
  perhaps the `scope` - see below;
- it reads the file (up to a limit, like `read_file`, but binary), copies it into the artifact store
  and returns the artifact's reference; the message carrying the call shows it as a download;
- it changes nothing in the workspace, but it hands content to people: approved where changes are,
  and the approver reads what is published under which name;
- the artifact is classified as the workspace it came from (169's fixed container), and published
  only where that label may flow into the channel - as reading the file there already requires.

### Open questions

1. **A, B or C** - and with it, whether PLAN's "one store" is revised.
2. **Who chooses the scope.** The agent (a `scope` parameter), only the session's scope with the
   user promoting an artifact later ("keep" → their scope), or the template. Recommended: the agent
   publishes into the session's scope; a user keeps an artifact by moving it into their own scope
   (or a global one), as an act of theirs - the transfer out of a volatile session asked for.
3. **Whose it is.** An artifact belongs to a user, as a workspace does: the agent's owner who
   published it, or the session's owner? Who may download it: whoever may read the message it was
   published at; in a user's or the global scope also its owner from outside the session.
4. **Relation to 208.** With B, 208's "where it is stored" is answered by this issue, and 208
   keeps only taking in A2A and MCP parts as artifacts. Its history should say so once this is
   decided.
5. **Reclaiming workspace space** is not part of this; it becomes possible without loss once
   results live elsewhere, and is an issue of its own when wanted.

## Plan

Proposed 2026-10-09, along the way `Workspace` is built (issue 253) - every step below has its
counterpart there. To be confirmed before code is written; the open points are at the end.

### The model (`caelumora-model/src/artifact.rs`)

```rust
define_id!(ArtifactId);

pub struct Artifact {
    pub id: ArtifactId,
    #[serde(flatten)]
    pub name: ScopedName,          // unique in its scope (issue 250); a file name, at most 100 characters
    pub storage: String,           // the configured artifact storage, fixed when it is made
    pub description: Option<Description>,
    pub classification: Classification,  // the workspace's it was first published from, fixed
    pub versions: Vec<ArtifactVersion>,  // oldest first, never empty; none is removed but with the artifact
    pub security_attributes: SecurityAttributes,  // owned by the user the agent acts for
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted: Option<Removal>,
}

pub struct ArtifactVersion {
    pub number: u32,               // 1, 2, ...; its content's key is `<artifact id>/<number>`
    pub size: u64,
    pub digest: Digest,
    pub media_type: Option<String>, // where its producer says (208); `publish_file` says none
    pub published_in: (SessionId, ChannelId),
    pub published_by: ActorId,     // the agent
    pub call: Option<String>,      // the tool call that published it
    pub published_at: DateTime<Utc>,
}
```

- `Stored` (`validate`: owned by a user, no contributors, in a session's scope, its owner's or the
  global one - as a workspace), `Deletable`, `Secured`, `Scoped`; `EntityKind::Artifact`.
- Versions live in the document: a handful of small records, read and written with it; a version
  of its own as a document would need its own lifecycle for nothing.

### The stores

- `EntityStore`: `create_`, `get_`, `update_`, `delete_`, `list_artifacts`, `restore_`, `purge_`,
  `get_deleted_`, `list_deleted_artifacts`, `find_artifact_by_name` - the memory and the JSON store
  through their generic helpers, the JSON store under `runtime/artifacts`; entries in the store
  suite (`lifecycle!`, `found_by_name!`, `unique_by_name!`).
- `ArtifactStorage` (`caelumora-runtime/src/artifact/storage.rs`): `put(key, stream, limit)`,
  `get(key) -> (size, stream)`, `delete(key)`, `remove_all(artifact)`, `destination()`; a `local`
  implementation keeping `<root>/<artifact id>/<number>`, written to a part file and renamed, as
  workspace files are.

### Security

- `ActionType::GetArtifact`, `ListArtifacts`, `CreateArtifact`, `UpdateArtifact`,
  `DeleteArtifact`; `ReadDeleted`, `Restore`, `Purge` with `EntityKind::Artifact`. The ownership
  policy decides them as for workspaces: the owner reads and changes, only the system purges.
- **Who downloads a version**: its owner, through `GetArtifact`; anyone else only through the
  channel it was published in - whoever may read that channel's history (`GetChannel`, through the
  channel's view, as the history is read) - and only where the version's label may be seen there,
  which it may since it was published there and labels are fixed. So a version is reached two
  ways, as a workspace is: owned (`/artifacts/...`) and through its channel
  (`/sessions/.../channels/.../artifacts/...`).
- Security suite: publishing (asks what a call on the workspace asks, plus `CreateArtifact` or
  `UpdateArtifact` in the owner's name), downloading by the owner, by a reader of the channel, not
  by someone who may not read it, not a version through a channel it was not published in; keeping;
  deleting with the session; a storage refusing a label above its `destination`.

### Runtime

- `ArtifactManager` (`caelumora-runtime/src/artifact/mod.rs`), owned by the `RuntimeManager`, built
  from `ArtifactsConfig`: `publish(ctx, workspace, path, name, description, channel, call)` - finds
  the artifact of that name in the session's scope or makes it, refuses a label other than the
  first version's, streams the file from the workspace's terminal into the storage, adds the
  version -; `content(ctx, id, number)`; `keep(ctx, id, scope)` - from the session's scope into
  its owner's or the global one, a name taken there refused -; `delete`, `restore`, `purge`,
  `purge_lying_in` (with the session, as `WorkspaceManager::purge_lying_in`).
- Events: `ArtifactPublished(id, number)`, `ArtifactUpdated` (kept, described), `ArtifactDeleted`,
  `ArtifactRestored`, delivered under `GetArtifact` - and a channel's readers learn of a version
  from the message that published it, as now.
- `scoped.rs`: a loop for artifacts in `delete_scoped_in` and `restore_scoped_in`, and
  `purge_scoped_in` hands them to `ArtifactManager::purge_lying_in`.
- Views: `ChannelView::artifact(id, number)` for the channel's route; `Views::artifacts()` and
  `deleted_artifacts()` for the owner.

### Configuration

```yaml
artifacts:
  max_bytes: 52428800          # the largest version; 50 MiB
  publish_to: files            # the storage new artifacts are made in; needed when there are several
  storage:
    files:
      provider: local
      root: "local/artifacts"
      destination: { level: confidential }   # optional, as for workspace storages
```

### `publish_artifact`, beside `publish_file`

Two tools, both offered: `publish_file` stays what issue 273 built - a link to the file of the
workspace as it is now, cheap, read-only -, and `publish_artifact` publishes a fixed copy. The model
is told the difference in their descriptions: a link for a result to look at now, an artifact for
one to be kept.

- Parameters as `publish_file`'s: `path`, `name`, `description`. It finds the artifact of that name
  in the session's scope or makes it, and adds a version; its result is
  `{artifact, version, name, size, digest, description, path}`.
- It changes the store: approved where changes are (`"unless_read_only"` asks); the approver reads
  "Publish 'out/r.csv' as artifact 'Report.csv', version 2".
- Presented as `artifact_publish`: a card like `publish_file`'s - sharing its layout - with the
  version, a download of that version through the channel's route, and "Keep" for its owner while
  it lies in the session's scope.
- **Which component offers it**: components are not given the runtime's services when they are
  built; a tool reaches the workspace through the turn's views when it is called, and
  `publish_artifact` reaches the `ArtifactManager` the same way. Without an artifact storage
  configured, a call fails and says so. It can be offered by `terminal_tool`, beside the file tools
  it reads from, or by a component of its own (`artifact_tool`) that a personality includes only
  where artifacts are wanted - decided: its own component, `artifact_tool`.

### REST

| Route | What |
|---|---|
| `GET /artifacts`, `?deleted=true` | the user's own artifacts, or their trash |
| `GET /artifacts/{id}` | one, with its versions |
| `GET /artifacts/{id}/versions/{n}/content` | a version's content, as the owner |
| `PUT /artifacts/{id}/scope` `{scope: user\|global}` | keep it beyond its session |
| `PUT /artifacts/{id}/description` | describe it |
| `DELETE /artifacts/{id}`, `POST …/restore`, `POST …/purge` | as for workspaces |
| `GET /sessions/{id}/channels/{cid}/artifacts` | the artifacts with a version published in the channel |
| `GET /sessions/{id}/channels/{cid}/artifacts/{aid}/versions/{n}/content` | a version's content, as a reader of the channel |

A download is always an attachment, `application/octet-stream`, `nosniff`, `no-store`, as a
workspace file's.

### Web UI

- The `artifact_publish` card shows the artifact's name, version and size, downloads the version,
  and offers its owner "Keep" (into their scope) while it lies in the session's.
- The list of a user's kept artifacts, with their trash and a page of versions, is a later step.

### Steps

1. **Model and stores** - `Artifact`, the store methods in both stores and the suite, the actions
   and their policy, the scoped cascade; no behaviour visible yet.
2. **Storage and manager** - `ArtifactsConfig`, `ArtifactStorage` with `local`, `ArtifactManager`,
   events, views; the security suite.
3. **`publish_artifact`** - the component `artifact_tool` and its tool over the manager, its
   approval and description.
4. **REST** - the routes, error mapping, `REST-SERVER.md`.
5. **Web UI** - the `artifact_publish` card with "Keep"; the list of kept artifacts is a later step.
6. **Documents** - `STORAGE.md`, `CONFIGURATION.md`, `TOOLS-AND-COMMANDS.md`, `SECURITY.md`,
   `WEB-UI.md`, `examples/config.yaml`.

### Open points

Decided 2026-10-09 (see History): both tools side by side; a label other than the first version's
refused; `artifacts.publish_to`; the card with "Keep" only in this issue; `publish_artifact` offered
by a component of its own, `artifact_tool`; an artifact's name at most 100 characters
(`publish_file` keeps its 255). None is open.

## Done when

An agent offers a file of the workspace for download with one call; the user downloads it from the
message that offered it, and it stays what it was when offered, whatever becomes of the file and
the workspace; its scope decides how long it is kept; the security suite covers publishing and
downloading; the documents say so.

## History

- **2026-10-09** — Opened at the maintainer's request: a `publish_file` tool, and artifact stores
  with a lifetime of their own on three levels - global, user, session - so that results are not
  lost when workspaces are cleaned up. The first question raised: whether artifact stores hang on
  the session or on the workspace.
- **2026-10-09** — Decided with the maintainer: **B**, artifacts as a kind of their own, published
  as fixed copies; PLAN.md's "one store" is revised for the reasons above - the workspace is the
  mutable working area, the artifact the fixed result. In detail:
  - **Metadata in the `EntityStore`**: an `Artifact` document in `caelumora-model` - name, media
    type, size, digest, classification, producer (agent, call), channel and message, scope,
    `SecurityAttributes`, `deleted` -, stored, filtered by permission, listed, deleted, restored
    and purged as a `Workspace` is. Not beside the objects in the storage: listing, permission
    filters and the trash are the entity store's.
  - **Content in an `ArtifactStorage`**: a trait in `caelumora-runtime` - put, get as a stream,
    delete, by key, no permissions -, one per storage configured under `artifacts.storage`, as
    `workspaces.storage` names workspace storages; the document names the storage it lies in.
    Named `…Storage`, not `…Store` (the stores keep documents, with a security context) nor
    `…Provider` (facets). Only a `local` provider for now; object storage is issue
    [272](272-artifacts-in-s3.md).
  - **An `ArtifactManager`** in the runtime asks the permissions and keeps document and content
    together, as the `WorkspaceManager` does.
  - **Keyed by the artifact's id**, its digest kept in the document to check the content: no
    deduplication, so that purging one artifact deletes its content without counting references.
    Deduplication can come later.
  - **Downloads are streamed through the server**, permission asked on every request, as for
    workspace files (attachment, `nosniff`); no presigned links.
  - **A storage takes nothing above its `destination`**, as a workspace storage does (issue 251,
    part B2).
  - **Scope:** the agent publishes into the session's scope alone; a person keeps an artifact by
    moving it into their own scope or the global one, an act of theirs.
  - **Owner:** the user the agent acts for (issue [255](255-whose-name-agents-act-in.md)).
    Whoever may read the message it was published at may download it; in a user's or the global
    scope, its owner also outside the session.
  - **Classification:** the workspace's it was copied from, fixed when it is published.
  - **208** keeps only taking in A2A and MCP parts as artifacts, through this issue's store.
  - **A cheap step comes first**, issue [273](273-publish-file.md): `publish_file` offers a file of
    the workspace as a link to it as it is now, with its digest, downloaded through the channel's
    workspace. This issue builds on it and keeps its tool, parameters, presentation and card;
    what changes is what lies behind them - publishing copies the file into an artifact, and the
    card downloads the artifact.
  - **Versions:** publishing again under the same name in the same scope makes a new version of
    the artifact, the earlier ones kept and downloadable, as artifacts on claude.ai and in Claude
    Code keep theirs. Artifacts that are rendered and edited in place, as claude.ai's are, are a
    step beyond this issue and an issue of their own when wanted.
- **2026-10-09** — Plan written (above); decided with the maintainer on it:
  - **Both ways side by side**, as two tools: `publish_file` stays a link to the file of the
    workspace (issue 273), and `publish_artifact` publishes a fixed copy. This replaces what was
    recorded above - that this issue would keep `publish_file`'s tool and only change what lies
    behind it.
  - Republishing with a label other than the first version's is **refused**.
  - New artifacts go to the storage **`artifacts.publish_to`** names.
  - The web UI of this issue is **the card with "Keep"**; the list of kept artifacts comes later.
- **2026-10-09** — Decided as well: `publish_artifact` is offered by a component of its own,
  `artifact_tool`, which a personality includes where artifacts are wanted and where later tools on
  artifacts go; an artifact's name is a `Name`, at most 100 characters. Status: in progress.
- **2026-10-09** — Weighed with the maintainer: artifacts belong to the `EntityStore`, not the
  `StateStore`. The state store keeps the state *of* a channel or an agent - by its holder's id,
  without a security context of its own, without names, scopes or trash, journaled because it
  changes often, gone with its holder. An artifact has an identity of its own: its own owner and
  permissions (`GetArtifact`), a name unique in its scope, a trash, and it leaves the session it was
  published in when it is kept - so it cannot be the state of a session or a channel. Its versions
  are few and added rarely; its content is in neither store. It would be channel state only if it
  could never leave the session and were read through the channel alone.
- **2026-10-09** — Steps 1 and 2 done:
  - `Artifact` and `ArtifactVersion` in `caelumora-model`; the store methods in both stores and the
    store suite; `Get`/`List`/`Create`/`Update`/`DeleteArtifact` decided as for workspaces;
    `AccessQuestions::may_change_artifact`. Without the `call` of a version the plan named: a tool
    is not told the id of its call, and a field always empty is left out until a producer fills it.
  - `ArtifactsConfig` (`max_bytes`, `publish_to`, `storage`), `ArtifactStorage` with `local`,
    `ArtifactManager` (`publish`, `content`, `published_content`, `keep`, `delete`, `restore`,
    `purge`, `purge_lying_in`), the events, the cascade with the session in `scoped.rs`, the views'
    edges `WorkspaceView::publish_artifact` and `ChannelView::artifact_content`. A version is read
    through the channel only by who reads it as a whole: whether an artifact's label may reach the
    place of an agent that reads it entry by entry is not asked yet.
  - Two steps that remove what would have been written twice: `scope::validate_users_own`, the
    rule a workspace and an artifact share, and `terminal::write_whole`, the writing through a part
    file that local workspaces and local artifacts share.
  - Security suite:
    `an_artifact_is_published_in_a_channel_for_its_publisher_and_read_by_who_reads_the_channel`.
- **2026-10-09** — Step 3 done: the component `artifact_tool` with `publish_artifact` (`path`,
  `name`, `description`), approved as a change unless configured otherwise, presented as
  `artifact_publish` (`ToolPresentation::ArtifactPublish`); its result is `artifact`, `version`,
  `name`, `size`, `digest`, `path` and `description`, classified as the artifact. The way to the
  workspace of the turn's channel is shared with `terminal_tool` (`tool::channel_workspace`), and so
  is how a terminal's errors are told to the model. The security suite's helper for a call on the
  workspace takes the component and when the call has arrived (`asked_for_a_call`);
  `an_agents_artifact_is_published_for_its_owner`.
