# 427 — The `local` storage of workspaces is part of the runtime crate

**Status:** rejected
**Type:** refactoring
**Area:** Code structure
**Summary:** The `local` storage of workspaces, whose shell runs commands on Caelumora's host as its user, is built into the runtime crate, so no deployment can be built without it and the core names one storage of its own beside those of plugins.

## The problem

The storage `local` - a directory per workspace under a root on Caelumora's host, with an opt-in
shell that runs `bash -c` there as Caelumora's user - lives in `caelumora-runtime/src/terminal/`:
`local.rs` (the provider and its `Terminal`, about 500 lines with tests) and `local_shell.rs` (its
`Shell`, about 340). The core registers it in `Backends::built_in` through
`terminal::register_backends`. The two Open Terminal storages left the runtime in issue
[327](327-open-terminal-in-a-crate-of-its-own.md), which kept `local` because "tests and
development need it and the artifacts' local storage shares `write_whole` with it".

Since then the terminal's API is public and complete (issue 327): `TerminalProvider`, `Terminal`,
`Shell`, `Commands`, `Kept`, `CommandKey`, `max_run`, `default_max_run_secs`. A provider outside
the core can be written against it, as `caelumora-open-terminal` shows; `local` is the one that is
not.

## Why it matters

- **A host shell is built into every deployment.** Whether agents may run commands on Caelumora's
  host is a question of configuration alone (`shell: true`). A deployment that must never offer
  it cannot leave it out of the build, as it can leave out `open_terminal_docker`.
- **The core names a storage.** Every other sandbox is a plugin; `local` is the exception, and
  `docs/WORKSPACE.md` has to explain why.
- **Dependencies.** `nix` and tokio's `process` feature are used by `local_shell.rs` alone.

The gain is mostly structural; nothing is broken, hence the priority.

## Considerations

- **The tests are the real work.** `local` is what the core's own tests run on:
  `caelumora_runtime::testing` (feature `test-support`) configures two `local` storages, one with a
  shell, for the world of every security suite - the runtime's, the agents', the artifacts' and
  the skills'; `workspace/testing.rs` (`session_with_workspace`) and the tests of
  `workspace/reaper.rs` start real commands in it; tests read the workspace's files on the host
  through `WithWorkspace::dir` and `files.path()`. A crate `caelumora-local-terminal` depends on
  the runtime, so the runtime's library - and `testing` is part of it - cannot depend on it.
  As a dev-dependency it serves the runtime's integration tests (`tests/`), not its unit tests,
  which would see the runtime twice. Options:
  - **A. The world takes its storage from the caller.** `testing` names no storage; every suite
    passes the plugin and its storage configuration, the runtime's integration tests through a
    dev-dependency on the new crate. The reaper's unit tests and `session_with_workspace` move to
    integration tests. Little new code, but every world is built with a plugin it does not test,
    and the core's unit tests can no longer reach a workspace at all.
  - **B. A memory storage for tests.** `testing` brings a `TerminalProvider` of its own that keeps
    files in memory and has a scripted shell - commands whose output and end the test sets, that
    run until killed - as `MemoryStore` stands beside the real stores. The core's tests then run
    no processes on the host and depend on no plugin; what only a real host shows (process groups,
    the cleared environment, links out of the workspace) is tested in the new crate, as now in
    `local.rs` and `local_shell.rs`. The agents' terminal tools, which run real commands in their
    tests, take the new crate as a dev-dependency. More code - the memory provider and its shell,
    and the tests that read files through `dir()` rewritten to read them through the terminal -
    but the core's tests describe the core.
  - **C. Keep `local` in the core**, as issue 327 decided, and say why in `docs/WORKSPACE.md` -
    as it does now.

  Recommendation: **B**. A is cheaper but makes every test of the core depend on a plugin, which
  is the dependency the move is meant to remove; C leaves the host shell in every build.
- **The terminal tool does not take part.** Moving `caelumora-agents/src/tool/terminal_tool/` into
  a crate of its own as well would not help: it uses the terminal's public API alone (`Terminal`,
  `Shell`, `WorkspaceManager`) and names no storage, and the agents can take the new crate as a
  dev-dependency without a cycle. The dependency in question is the runtime's own, of its
  `testing` world and its unit tests. Whether the tool belongs in a crate of its own is a question
  of its own - it reaches into the agents' `Turn`, `Situation` and tool helpers.
- **`write_whole`** is shared by the files of a `local` workspace and the local storage of
  artifacts (`caelumora-artifacts/src/local.rs`). It is a helper of writing a host file whole on
  `ByteStream`s, not part of the `local` storage, and stays in `caelumora_runtime::terminal`
  beside `ByteStream`, moved out of `local.rs`; the artifacts do not come to depend on the new
  crate.
- **What becomes public for it:** `WorkspacePath::under` (a path below a host directory, which a
  provider with a directory per workspace needs) and `commands::locked`, which `local_shell.rs`
  takes its locks with - kept private in issue 327 because Open Terminal did not need it; a copy
  in the new crate would repeat it.
- **Names.** The crate `caelumora-local-terminal`, after `caelumora-open-terminal`; its plugin
  `LocalTerminalPlugin`, named `local_terminal`. The storage's `type` stays `local`, so that no
  configuration changes; the programs list the plugin.
- **`Backends::built_in`** then registers no storage of workspaces; a configuration naming `local`
  without the plugin fails as one naming an unknown type does.

## Done when

- `caelumora-local-terminal` holds the `local` provider, its terminal and its shell with their
  tests; `LocalTerminalPlugin` registers `local` in `Plugin::backends`.
- The runtime no longer depends on `nix` nor on tokio's `process` feature, and compiles and passes
  its tests without the new crate.
- `caelumora-app` and `caelumora-tui` list the plugin; storages configured as `type: local` work as
  before.
- `docs/WORKSPACE.md` and `docs/ARCHITECTURE.md` no longer name a storage the core brings, and say
  how the core's tests reach a workspace.

## History

- **2026-10-18** — Opened from the maintainer's question whether the `local` storage could move
  into a crate of its own. Feasible: the terminal's API is public since issue 327. The decision
  between options A, B and C for the tests is open.
- **2026-10-18** — Rejected. Option B would implement the terminal's contract a second time for
  the tests alone - its files in memory beside `local`'s on disk, which could drift apart unseen -
  and fake the shell where the reaper's tests are to show that real processes end. Option A avoids
  that but rebuilds the tests of every suite around a plugin, and leaves the runtime's unit tests
  without a workspace. Neither is worth what the move gains: `local` stays in the core (option C),
  as issue 327 decided. Moving the terminal tool into a crate of its own would not have helped
  either, since the dependency in question is the runtime's own.
