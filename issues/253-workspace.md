# 253 — Agents and people have no workspace to keep files in

**Status:** done
**Area:** Context and knowledge
**Summary:** Nothing in Caelumora holds a file: agents and people have no workspace to keep, share and work on files in, and an executor would have to own them.

## The problem

Nothing in Caelumora holds a file. A session is a conversation: what an agent produces is text in a
message, what a person brings in is text in a message, and what both would work on - a document to
revise, data to analyse, code to build and run - has no place. The fragments that assume one are
listed in [PLAN.md](PLAN.md) (block B1): artifacts that are dropped (issue
[208](208-generated-artifacts.md)), the rule for changing something classified (issue
[169](169-changing-classified-content.md)), the term *workspace* in MEMORY.md's table of tiers, and
Open Terminal (issue [251](251-open-terminal.md)), which brings a container with files and a shell
but no files of Caelumora's own.

## Why it matters

A workspace is what turns a conversation into work: files that outlive a turn, that several people
and agents of a session read and change, and that an executor can run things on. Without one, every
tool that produces more than text has nowhere to put it, and every executor becomes the owner of
the files it was only meant to run on.

## What is wanted

- A **workspace** belongs to a user, is attached to sessions, and is one tree of files with one
  classification (169's fixed container) and the ownership of every stored kind: an owner and
  contributors, a trash, a name unique per user.
- It lies in a *storage*, a named place configured once, served by one `TerminalProvider` that
  makes its place and hands out its `Terminal`: the one handle through which its files are read
  and written and, where the terminal has a `Shell`, commands run - a local directory, an Open
  Terminal instance, a container per workspace. Everything that works on a workspace holds that
  handle; the tools of agents are generic over it (`terminal_tool`).
- A session has at most one workspace, shared by all its channels.
- People upload, download, list and delete files in the web UI; a channel uses the workspace only
  where its classification flows both ways; agents learn which workspace they have.

## Considerations

The reasoning - who may do what, what may flow, where the directory is, what the views and the
REST API offer, how the web UI shows it, and the steps - is in issue 251's plan, parts B, E, F and G
([251](251-open-terminal.md#plan)): the workspace was decided together with its first executor, and
its plan is kept in one place so that the two are read together. In short:

- **One kind, built like every kind:** `Workspace` in `caelumora-model`, with `Scoped` in its
  owner's `UserScope` and the name of its storage; `Session::workspace` names the one attached;
  nothing in a template.
- **Two questions apart:** who may - the five actions of the kind, contributors *use* as they write
  into a session; what may flow - the workspace is one container classified W, a person cleared for
  W as for a channel, a channel L using it only where W flows into L and L into W, asked as the
  flow questions the policy has.
- **One owner in the runtime:** a `WorkspaceManager` held by the `RuntimeManager`, the one writer
  of the documents and the only code that holds the providers - one per storage; the views reach
  it (`WorkspaceView`, with `terminal()` after the check) for people and agents alike; attaching
  is the session's. A storage's `destination` bounds the classification a workspace in it may
  have.
- **Not in it:** versions and a state to go back to (W4), several workspaces per session,
  artifacts (208). The workspace's own file tools (W2) are a step of 251's plan.

## Done when

- A user creates a workspace, shares it with contributors, attaches it to a session, uploads and
  downloads files in the web UI, and deletes, restores and purges it; the security suite covers
  every act.
- A session's agents are told which workspace they have and whether they may use it in their
  channel, and a toolbox runs on the same files a person uploaded.
- STORAGE.md, SECURITY.md, CONFIDENTIALITY.md, CHANNELS.md, CONFIGURATION.md, REST-SERVER.md and
  WEB-UI.md describe it; ARCHITECTURE.md names it in the domain.

## History

- **2026-10-08** — Opened from the plan of issue [251](251-open-terminal.md), where the workspace
  was decided as a thing of Caelumora's own - a user's, attached to sessions, not bound to Open
  Terminal - and planned with its executor. It is PLAN.md's W1 and W3.
- **2026-10-08** — Reshaped with 251's plan: three layers (storage, access, toolbox), at most one
  workspace per session.
- **2026-10-08** — Reshaped again with 251's plan: one `TerminalProvider` per storage, the
  `Terminal` as the one handle for files and commands, the file tools a step of `terminal_tool`.
- **2026-10-08** — The workspace in the model (251's plan, step 2): `Workspace` and `WorkspaceId`
  (`caelumora_model::workspace`), `Stored` with `validate` - a workspace belongs to a user -,
  `Deletable`, `Scoped`; `EntityKind::Workspace`; the actions `GetWorkspace`, `ListWorkspaces`,
  `CreateWorkspace`, `UpdateWorkspace`, `DeleteWorkspace` under the ownership policy; the ten
  store methods in both stores (`runtime/workspaces/` of the JSON store), checked by the store
  suite - lifecycle, found by name in the owner's scope, unique by name per owner, refused without
  a user as owner; `Session::workspace` and `SessionUpdateEvent::WorkspaceChanged`, applied by the
  session's controller; the four `Workspace…` events, received by whoever may read the workspace.
  Different from the plan: the name is a `Name`, not a `ScopedName`; the scope is derived from the
  owner, as a channel's is from its session, so that the two cannot disagree. Nothing uses it yet.
  Stored data: none to delete - every new field has a default.
- **2026-10-08** — Step 3 of 251's plan: the workspace works, without a shell. Decided before it
  (251, *Decisions* 10 to 12): a workspace is used in a session through its channels and has no
  contributors - `WorkspaceUpdateEvent::ContributorsChanged` is gone again, `validate` refuses
  contributors; `Channel::uses_workspace` and `ChannelTemplate::uses_workspace` (on by default),
  `ChannelUpdateEvent::UsesWorkspaceChanged`, `ChannelLoop::update_uses_workspace`. The section
  `workspaces` of `AppConfig` (`max_file_bytes`, `storage` with the provider `local`). In the
  runtime: `terminal` - `TerminalProvider`, `Terminal`, `WorkspacePath`, `TerminalError` and the
  `local` provider, which follows a link only inside the workspace and writes through a part file -
  and the `WorkspaceManager`, held by the `RuntimeManager` (`RuntimeManager::new` takes the
  section): create with its place, rename, describe, reclassify, delete, restore, purge with its
  place; `owned` and `usable_in`, the two ways of using one. `SessionLoop::set_workspace`. The
  views: `Views::workspaces`, `workspace`, `deleted_workspaces`, `ChannelView::workspace`,
  `WorkspaceView` with `terminal()`, which asks again of the session and the channel as they are
  then - a first version asked of the documents as they were when reached, which the security suite
  caught. The security suite: owner alone, attaching, use in a channel where both flow and the
  switch is on, not by who is not in the channel. Documented in CONFIGURATION.md, CHANNELS.md,
  CONFIDENTIALITY.md, SECURITY.md, STORAGE.md and ARCHITECTURE.md. `scoped.rs` needed nothing:
  nothing deletes what lies in a user's scope yet. Stored data: none to delete - every new field has
  a default.
- **2026-10-08** — Step 3b (251, *Decisions* 13 and 14): `Workspace::name` is a `ScopedName` again,
  in a session's scope, its owner's or the global one (`validate`: not a channel's or a group's,
  no other user's), so the deviation of step 2 is taken back. `SessionTemplate::workspace`, a
  `WorkspaceTemplate`, found or made by `create_from_template` (`template::resolve_workspace`) and
  attached, taken back if the session cannot be created; `TemplateError::WorkspaceNameTaken` for
  one of its name in another storage. What lies in a session's scope is used in it alone:
  `SessionError::WorkspaceOutOfScope` when attached elsewhere, refused to its owner outside it. The
  `WorkspaceManager` deletes, restores and purges what lies in a scope, and `scoped.rs` hands it
  a session's. `WorkspaceManager::create` takes the scope. The suite: a workspace of a session's
  scope made with each session and gone with it, files included; one of the user's scope shared by
  their sessions. Stored data: workspaces stored before load in the global scope, their names
  unique across users - delete `runtime/workspaces/` of a JSON store, and their places.
- **2026-10-08** — Deleting and restoring the workspaces of a session's scope no longer go through
  the `WorkspaceManager`: `scoped.rs` marks them in the store and announces them, as it does
  agents. Only purging needs the manager, which removes the places first (`purge_lying_in`);
  `delete_lying_in` and `restore_lying_in` are gone. No change of behaviour.
- **2026-10-08** — Step 4 of 251's plan: REST (`routes/workspaces.rs`, see REST-SERVER.md) and the
  web UI - the user's workspaces in the sidebar with a page of their own in the place of a channel,
  `FileBrowser` over either way to the files, attaching from a session's menu, and "Files" with the
  channel's switch in the channel panel; `App.tsx`'s layout component is `Layout` now. Checked
  against a running server: with `curl` - a 3 MiB file back unchanged, `..` refused also encoded, a
  stranger shown nothing, the channel's way and its switch - and in the browser through the DOM;
  screenshots could not be taken, the browser window being hidden. Downloads are
  `application/octet-stream` for now: `mime_guess` would have been a new dependency.
- **2026-10-08** — Step 5 of 251's plan: a storage may have a shell (`StorageConfig::Local::shell`)
  and bounds what it takes (`destination`): `WorkspaceManager::create` and `reclassify` refuse a
  workspace above it (`WorkspaceError::StorageNotCleared`, asked as `Publish` into the
  destination); snapshots and the REST responses say `has_shell`. Stored data: unchanged.
- **2026-10-08** — Done with 251's plan. As *Done when* asks, but for sharing with contributors: a
  workspace has none, its owner's alone outside a session and shared through the channels of the
  session it is attached to (251, decision 12). Agents use it with `terminal_tool` - its files and,
  where the storage gives one, a shell - on `local`, `open_terminal_shared` and
  `open_terminal_docker` storages. Versions are issue [256](256-workspace-versions.md); in whose
  name an agent reaches it, issue [255](255-whose-name-agents-act-in.md).
