# Workspaces

A **workspace** is a tree of files a user owns, which agents and people work on together: attached
to a session, used by its channels, and served by a storage that may also run commands in it. This
document describes what a workspace is, how a session and its channels come to use one, the storages
its files lie in and how far they keep workspaces apart, and the commands run there. Who may read
and change it is in [SECURITY.md](SECURITY.md#ownership), who is cleared for it in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#workspaces), the tools agents use on it in
[TOOLS.md](TOOLS.md#the-workspace), and its storages are configured in
[CONFIGURATION.md](CONFIGURATION.md#workspace-storages).

## What a workspace is

A workspace is an entity of the `EntityStore` (see [STORAGE.md](STORAGE.md)): a document with a
name, an owner - always a user, also where it lies in another's scope - an access list and one
classification for all its files, which carry none of their own. The document names the storage its
files lie in; the files are no part of it.

It lies in a session's scope, its owner's or the global one - never a channel's, since a session's
channels share one. Its scope decides its life: one in a session's scope is made for that session,
used within it alone, and deleted, restored and purged with it, its files too; one in its owner's
scope or the global one outlives every session that uses it.

The `WorkspaceManager` is the one writer of workspace documents and the only code that holds the
storages. A workspace is made with its place in the storage and purged with it, the place first, so
that a place that cannot be removed leaves the document for the next purge.

## In a session

A session has at most one workspace attached, shared by its channels. A template may name it -
found by its name in the scope the template asks for, or made there by the session's owner, as an
agent of a template is. Whoever makes a session may choose instead: none, one of their own, or one
made for the session alone. Otherwise only the workspace's owner attaches it, to a session they may
change - not one of another session's scope; whoever may change the session takes it away.

**Attaching shares nothing by itself**: who may use a workspace is its access list's to say (see
[SECURITY.md](SECURITY.md#access-lists)). One made for a session lets the session's members read and
write it; one of its owner's grants nobody anything until they share it - when attaching it, or by
its list afterwards. Taking it away leaves the list as it is: it is the workspace's. An entry for a
session that was purged is dropped as the workspace is next stored (issue
[347](../issues/347-entries-naming-purged-groups.md)).

A person reaches the files by the workspace, wherever they act - never through a channel: as its
list lets them, and cleared for its classification. An agent uses it through a channel, acting with
its owner's rights for whoever asked it, as the list lets the agent, and only where the channel uses
it (see [CHANNELS.md](CHANNELS.md#its-workspace)). A channel uses it where

*   its switch says so (`Channel::uses_workspace`, on unless its template, its creator or whoever
    may change the channel says otherwise) - what the channel is for; and
*   the workspace and the channel flow both ways (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#workspaces)) - the boundary, which the switch cannot
    widen.

So a session for a request can have a channel with the person who asked, classified below the
workspace or with the switch off, and channels with helpers that work on its files. A view of a
channel offers the workspace only where it may be used there, and says nothing of it otherwise.
Using it in a channel is the `WorkspaceManager`'s: it reads the session and the channel as they are
stored then and asks as the views ask - whether the caller may read the channel, whether the
workspace is usable there, and for changing files `WriteWorkspace`.

## Storages

The files of workspaces lie in the configured storages, each served by one `TerminalProvider`,
which makes and removes the place of a workspace and hands out its `Terminal`, the one handle to its
files. A path a caller writes is made a `WorkspacePath` - relative, without `..` - before any
provider sees it.

The providers - `local`, `open_terminal_shared` (one Open Terminal instance started outside
Caelumora) and `open_terminal_docker` (a volume and a container per workspace, started by Caelumora)
- differ in what their backend does; how each meets the rules of this document is in their module
docs (`caelumora-open-terminal/src/terminal.rs` for Open Terminal). A storage's key is resolved
through the secret chain in the system's name, since the storage is the operator's.

A storage is a backend (issue 324, see [ARCHITECTURE.md](ARCHITECTURE.md#backends)): its provider
is made by the factory registered for its `type`, which reads and checks its parameters. The core
brings `local` alone, registered by `terminal::register_backends` beside its provider in
`caelumora-runtime/src/terminal/`, which tests and development need. The two Open Terminal storages
are the plugin `caelumora-open-terminal` (issue 327), which the programs list; the Docker one, with
`bollard`, is behind its feature `docker`, on by default, so that a deployment with one shared
instance builds without it. Another sandbox - a pod, a remote machine - is added the same way, by a
plugin that registers a `TerminalProvider` of its own.

What a provider has to offer is the public API of `caelumora_runtime::terminal`:

*   a `TerminalProvider`: making and removing the place of a workspace, its `Terminal`, whether it
    has a shell, the commands that may still run and ending one (`CommandKey`), and its
    `destination`;
*   the `Terminal` it hands out, on `WorkspacePath`s, with files as `ByteStream`s and its failures
    as `TerminalError`s, and, if it has one, its `Shell` for one scope - whose `end` ends a command
    as a provider is told to;
*   the commands of its storage, kept in `Commands` by what its shells keep of each (`Kept`), so
    that they are listed and forgotten as the core's are;
*   its parameter `max_run_secs`, read with `default_max_run_secs` and checked with `max_run`, so
    that every storage bounds its commands alike.

A storage bounds what may be put into it: its `destination`, the classification a workspace in it
may have at most - where its files go, and for a storage with a shell where what a command reads can
go, since a command reaches whatever its host or container reaches; making or raising a workspace
there is checked against it (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md#workspaces)). A `local`
storage with a shell is bounded to public unless it names its own; one without a shell, and without
a `destination`, is not bounded.

The isolation between workspaces is the storage's, not the classification's. A `local` shell runs on
Caelumora's host as its user, and reaches every workspace of every storage there, and Caelumora's
own files. An `open_terminal_shared` storage keeps all its workspaces in one instance, so a command
in one reaches every other, and whatever the instance's network reaches; its key, which Caelumora
blanks in what it starts, can still be read from the server's process and opens every command and
file of the instance. So: one classification per such storage, and a storage per classification
where there are several. An `open_terminal_docker` storage gives every workspace a container of its
own: a command reaches its workspace and the container's network, and its key opens that container
alone. A classification says who may read a workspace; it does not keep a command in it.

## Commands

A terminal may have a `Shell`, a capability of its storage: commands started in the workspace,
followed by what they print and killed with everything they started. A shell is made for one scope -
for the runtime, the channel - and sees only the commands started under it. A storage knows the
commands it started, so that none is started past it, and ends one when told, without asking
anything.

**Commands live with the storage**, not with a call or a turn: a command handed back to an agent as
running outlives the call that started it. A command is its channel's, not its starter's: it runs on
as long as some current member of the channel could start it there, and an agent removed from the
channel leaves its command to the members who remain. Whoever ends a command reaches the shell bound
to the channel (`WorkspaceManager::shell_in`), so that who may end a command is who could have
started it there.

A command nobody ends is ended by the runtime:

*   once it ran for its storage's `max_run_secs`;
*   once its session is stopped, or its workspace purged - removing a place ends the commands in it
    first;
*   once nobody may run it any longer where it was started. The `WorkspaceManager`, woken by the
    events that can change who may use a workspace in a channel and besides every minute, asks for
    each command whether some current member of its channel could start it there, in that member's
    name with the runtime as initiator. A command whose question cannot be answered for now is kept.

The list of commands lives in memory, so what a restart leaves running is named, not closed:
`open_terminal_docker` stops at start what an earlier run left; a command of an earlier run in an
`open_terminal_shared` instance ends only at `max_run_secs`; one of the `local` shell is not bounded
at all. A command that leaves its process group escapes the time bound of an Open Terminal instance.
