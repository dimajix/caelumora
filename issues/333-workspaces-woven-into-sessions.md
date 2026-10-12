# 333 — Workspaces are woven into sessions, channels and templates

**Status:** open
**Area:** Code structure

## The problem

Issue [323](323-kinds-enumerated-in-the-core.md) opened the core for plugins and moved skills
into `caelumora-skills`. Its plan foresees an issue for each further subject that might follow;
this is the one for workspaces (issues 251, 253). Unlike skills, which only the `skills` component
and its routes used, workspaces are part of what the core's own kinds are:

- **The session names one.** `Session::workspace` (`caelumora-model/src/session.rs`) is the
  workspace its people and agents share; attaching and detaching it is a session's update
  (`/sessions/{id}/workspace`).
- **A channel uses it or not.** `Channel::uses_workspace`, set per channel
  (`/sessions/{id}/channels/{cid}/uses-workspace`), and read by the tools and the prompt part.
- **A template makes one.** `SessionTemplate::workspace` (`WorkspaceTemplate`), which
  `caelumora-runtime/src/template.rs` turns into a workspace while it builds a session.
- **The terminal is its.** `caelumora-runtime/src/terminal/` - the local storages, the Open
  Terminal storages (issue [327](327-open-terminal-in-a-crate-of-its-own.md)), the commands run in
  a workspace and `CommandStatus` -, `terminal_tool` with its file and shell tools, and
  `workspace_prompt_provider` exist for workspaces alone.
- **Others depend on it.** `artifact_tool` publishes a workspace's file (issue
  [328](328-artifacts-in-a-crate-of-their-own.md)); the knowledge of a turn records the files it
  read (`KnowledgeSource::Entity` of the kind workspace); the verb `WRITE` is shared with
  artifacts.
- **The configuration and the errors name it.** `AppConfig::workspaces` with the closed
  `StorageConfig` (issue [324](324-backends-enumerated-in-the-core.md)),
  `ViewError::WorkspaceNotFound`, `WorkspaceError` in `user_message` and in `From<anyhow::Error>`
  of the REST server, the `WorkspaceManager` among the runtime's services.

## Why it matters

As long as workspaces are the core's, every program builds the terminal and its storages, and a
deployment without workspaces carries them anyway. But moving them is not a move of files, as
skills were: the session, the channel and the template would have to name what they hold without
knowing what it is.

## Considerations

What step 10 of issue 323 showed for a plugin, and what it means here:

- **The runtime's internals a plugin uses have to be opened** - for skills the helpers of
  `scoped`, `ApprovalMode`, `ApiError::bad_request`. Workspaces use far more of the runtime: the
  controllers, the channel's view, the session's settings, the turn's places.
- **A kind's errors become the plugin's** (`SkillError::NotFound`, `NoSkillNamed` instead of
  `ViewError` variants); `user_message` then no longer knows them (issue
  [336](336-errors-of-a-plugin-shown-as-a-failure.md)).
- **Tests that use the kind as an example in the core** have to use another kind, and the tests
  of the example seed and configuration live in `caelumora-app`.

Options:

- **A. Workspaces stay in the core.** The terminal and its storages can still leave as crates of
  their own (issues 324, 327), which takes most of what a deployment may not want. Cheapest.
- **B. A session holds an attachment of whatever kind** - `(EntityKind, id)` instead of
  `Option<WorkspaceId>`, a channel's `uses_workspace` as "uses the session's attachment", a
  template's workspace as a section of the template that a plugin claims, as the seed's sections
  are claimed. The workspace plugin then provides what a session's attachment is. Costly, and a
  concept with one instance.
- **C. First artifacts (issue 328), then decide.** Artifacts depend on workspaces; moving them
  first shows whether one plugin depending on another works, and what the core keeps.

Recommendation: C, with A as the likely outcome for the session's part, and the terminal moved
out through 324 and 327 either way.

## Done when

- It is decided whether workspaces leave the core, and recorded here with the reasons.
- If they do: `caelumora-workspaces` holds the entity, its manager, the terminal, its tools, its
  prompt part, its views, routes and tests; the core names no workspace; stored sessions,
  channels and templates are converted or deleted, as the summary says.

## History

- **2026-10-12** — Opened as a follow-up of issue 323 (step 11), with what moving skills showed.
