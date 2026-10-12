# 327 — Open Terminal is part of the runtime crate

**Status:** open
**Area:** Code structure

## The problem

The two storages of workspaces backed by Open Terminal (issue [251](251-open-terminal.md)) -
`open_terminal_shared`, one instance started outside, and `open_terminal_docker`, a container per
workspace started over the Docker daemon - live in `caelumora-runtime/src/terminal/open_terminal/`,
about 1760 lines with the REST client, its wire types, both providers and a stand-in for their
tests. The Docker provider brings `bollard` into the runtime.

Unlike MCP and A2A they are no component kind: a storage is a backend of the `WorkspaceManager`,
named in `StorageConfig` (`caelumora-model/src/config.rs`), a closed enum with a variant for each,
and made by a `match` in `WorkspaceManager::new`. `StorageConfig::max_run` and `check` match over
the variants as well, the checks of the Open Terminal variants included. The trait the providers
implement, `TerminalProvider`, is `pub(crate)`, as are `CommandKey` and the `Commands` a storage
keeps its commands in; `Terminal` and `Shell`, the handles it gives out, are public already.

## Why it matters

- `bollard` is, after `rmcp`, the heaviest dependency of the runtime, and is needed only where
  Caelumora starts containers itself.
- Open Terminal is one of several sandboxes a workspace could live in; another (a Kubernetes pod,
  a remote VM) would be added the same way, and should not have to edit the core.

## Considerations

- **Blocked by issue [324](324-backends-enumerated-in-the-core.md).** As long as `StorageConfig`
  names the variants, nothing outside the core can provide them. Once backends are registered, the
  crate registers `open_terminal_shared` and `open_terminal_docker` and owns their configuration
  types, `check` and `max_run` included.
- **The terminal's API.** `TerminalProvider` becomes public, with `CommandKey` and the `Commands`
  a storage keeps (issue 254), beside the `Terminal`, `Shell`, `WorkspacePath`, `ByteStream` and
  `TerminalError` that are public already. This is the larger part of the work, and the part to
  review: it fixes what a sandbox has to offer.
- **Docker as a feature.** One crate `caelumora-open-terminal`, with the Docker provider behind a
  feature `docker` (on by default), so that a deployment with one shared instance builds without
  `bollard`. Two crates would split a client both providers share.
- **The shared remote helpers** of `caelumora-runtime/src/remote.rs` are decided in issue
  [326](326-a2a-in-a-crate-of-its-own.md); this crate follows the decision.
- **What stays:** the `local` provider, which tests and development need and which the artifacts'
  local storage shares `write_whole` with.

## Done when

- `caelumora-open-terminal` holds both providers, their client and the stand-in; the runtime
  depends on `bollard` no longer, and compiles and passes its tests without the crate.
- The binaries list it; the storages configured as `open_terminal_shared` and
  `open_terminal_docker` work as before, their configuration unchanged unless issue 324 changed
  the tag.
- `docs/WORKSPACE.md` says where the providers live and what a provider has to implement.

## History

- **2026-10-12** — Opened from the discussion of which subjects besides skills could move into
  crates of their own (issue 323); found to need issue 324 first.
- **2026-10-12** — Issue 324 is done: the storages are backends (`Backends`), the tag stays
  `type`, and the crate registers `open_terminal_shared` and `open_terminal_docker` in
  `Plugin::backends`. Their parameters are types beside their providers already
  (`SharedStorageConfig`, `DockerStorageConfig`), and `TerminalProvider` and `CommandKey` are
  public; `Commands` and `Kept` are not yet. Still open before it: the remote helpers (issue 326).
