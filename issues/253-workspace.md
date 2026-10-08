# 253 — Agents and people have no workspace to keep files in

**Status:** open
**Area:** Context and knowledge

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
