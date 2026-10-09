# 266 — A new session's workspace cannot be chosen when it is made

**Status:** open
**Area:** User interaction

## The problem

"New Session" in the web UI makes a session from the template chosen in the sidebar's dropdown,
named after it, and nothing else (`Sidebar.handleCreateSession`, `POST /sessions` with
`template_id` and `name`). Its workspace is the one its template names, if any
(`SessionTemplate::workspace`, issue [253](253-workspace.md)); a different one can only be attached
afterwards through the session's menu (`SessionWorkspaceDialog`, `SessionLoop::set_workspace`).

A workspace for this session alone - one in the session's scope - cannot be made at all except by a
template: `NewWorkspaceDialog` offers the user's scope and the global one, since a session's scope
does not exist before the session.

## Why it matters

Which files a session works on is decided when it is begun, not afterwards: a user who starts a
session for a piece of work wants its workspace with it - one of theirs, or a fresh one for this
session alone. Attaching one later is a second step that is easily forgotten, and a fresh one
without a template made for it is not possible.

## Considerations

**Where the choice is made.** Two ways were weighed with the maintainer:

- **A second dropdown in the sidebar**, beside the templates. Fastest to use, but the sidebar is
  narrow, the two dropdowns depend on each other - a template may name a workspace already - and a
  fresh workspace needs a storage and a classification, which a dropdown has no room for.
- **A dialog that "New Session" opens.** Heavier by one step, but it shows the template's choice and
  what overrides it, has room for the storage and classification of a fresh workspace, and for what
  will come later (the session's descriptions, issue
  [245](245-session-and-channel-descriptions.md), its privacy).

The dialog is the way, kept light: every field filled in, so that "New Session" and Enter make a
session as today's button does.

- **Template** - the last one used, as now (`loadLastTemplate`).
- **Name** - "New \<template\>".
- **Workspace** - one of:
  - "As the template says", naming what the template would find or make, where it names one;
    "None" where it does not. This is the default.
  - "None", where the template names one.
  - each of the user's own workspaces - in their scope or the global one, not one of another
    session's scope (as `SessionWorkspaceDialog` offers them);
  - "New, for this session alone": then its storage - the kind of workspace, e.g. plain files or
    an Open Terminal sandbox with a shell - and its classification (`ClassificationPicker`).
    The storage is always offered, also where there is only one, so that the user sees what kind
    of workspace is made; it is preset to the template's where the template names one, else to the
    first the server names. The classification is preset as a template's is (`internal`).

**Only one's own workspaces.** Attaching is the workspace's owner's (`UpdateWorkspace`), also of a
global one (see [CHANNELS.md](../docs/CHANNELS.md#its-workspace)). Decided: the dialog offers the
user's own workspaces only; letting anyone attach a global workspace they may read is not part of
this.

**The choice overrides the template's.** Decided: a workspace chosen in the request takes the
place of the template's - the template's is then neither found nor made. Absent, the template's
applies as today.

**The server takes the choice when the session is made, not the web UI in two steps.** Making the
session and then calling `set_workspace` would need no change of the creation, but cannot make a
workspace in the session's scope - there is no REST path for it - and leaves a session without its
workspace when the second step fails. Instead `create_from_template` takes the choice, and
`CreateSessionRequest` carries it:

- absent - the template's workspace, as today;
- none - no workspace, whatever the template names;
- an existing one, by its id - checked as `set_workspace` checks it: the caller may read it
  (`GetWorkspace`) and attach it (`UpdateWorkspace`), and it is not of another session's scope
  (`SessionError::WorkspaceOutOfScope`). `UpdateSession` needs no check: the caller is making the
  session. The checks move into one function both use, rather than being written a second time;
- a new one in the session's scope - its storage and classification, made as a template's is made
  (`workspaces.create`, which checks the classification) and, like it, deleted again when the
  session cannot be created (`create_from_template` does so already). Its name: the session's,
  unless a better one is found; within the new session's scope no name is taken.

How the choice is typed - an enum next to `WorkspaceTemplate` in `caelumora-model`, or the
"new" case as a `WorkspaceTemplate` with a session scope - is decided when it is built; reusing
`WorkspaceTemplate` keeps one way to describe a workspace to be made.

**What the dialog needs to know.** The templates' list (`GET /templates`, `TemplateResponse`) gives
id, name and description only; to say "as the template says" it needs what the template names - the
workspace's name, its scope and its storage. The storages are named by the server already
(`GET /workspaces/storages`, `StorageResponse`, as `NewWorkspaceDialog` uses them), but by their
configured name alone; to tell a user what kind each is, the response could also say whether it
gives a shell, as a workspace's does (`has_shell`) - and `NewWorkspaceDialog` would show the
same.

**The security suite** gets a test of the new path: one's own workspace attached when the session
is made; another's refused (`UpdateWorkspace`); one of another session's scope refused; a new one
with a classification the user is not cleared for refused, and no session left behind by a refusal.

**Not part of this:** the session's descriptions and privacy in the dialog. It leaves room for them;
they come as issues of their own when wanted.

## Done when

"New Session" opens a dialog with template, name and workspace, preset so that Enter makes the
session as before; the server makes the session with the workspace chosen - none, one of the
user's own, or a new one for it alone - in one request, overriding the template's; the security
suite covers it; `CHANNELS.md`, `REST-SERVER.md` and `WEB-UI.md` say so.

## History

- **2026-10-09** — Opened after discussion with the maintainer: a dialog rather than a second
  dropdown; only the user's own workspaces offered; a workspace chosen overrides the template's; a
  new workspace for the session is made in a storage the user chooses, always offered.
