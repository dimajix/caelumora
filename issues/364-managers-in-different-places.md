# 364 — A module's manager lives in `manager.rs` in some modules, in `mod.rs` or a flat file in others

**Status:** open
**Area:** Code structure

## The problem

A manager is the one owner of the lifecycle of a kind - making, deleting, restoring, purging it.
Where it lives in its module differs from module to module:

| Manager | Where | Its error type |
|---|---|---|
| `AgentManager` | `agent/manager.rs`, the module public (`pub mod manager`) | `AgentError`, in `manager.rs`; imported as `agent::manager::AgentError` |
| `SessionManager` | `session/manager.rs`, private, `pub use manager::SessionManager` | `SessionError`, in `mod.rs` |
| `WorkspaceManager` | `workspace/mod.rs` (463 lines), beside the module's description | `WorkspaceError`, in `mod.rs` |
| `ControllerManager` | `controller/mod.rs` (447 lines), beside one file per controller | - |
| `GroupManager` | `group.rs`, a flat file (588 lines) | `GroupError`, in the same file |
| `ArtifactManager`, `SkillManager` | `manager.rs` of their plugin crates | - |
| `UserManager` (model) | `user_provider/mod.rs` (394 lines) | - |

Even the two modules that have a `manager.rs` already differ: `agent` opens the module, `session`
re-exports the type; the error type lies once with the manager, once in `mod.rs`.

## Why it matters

Whoever looks for the manager of a kind has to search for it, and a `mod.rs` that holds the manager
is no longer the overview of its module: its description, its error type and what it exports are
lost between the manager's methods and tests. The same concept is found in the same place
everywhere else (AGENTS.md, *A clear and consistent public API*).

## Considerations

- **A rule in CODING-GUIDELINES.md:** a module of a kind that has a manager keeps it in
  `manager.rs`, as `mod manager;` with `pub use manager::XyzManager;`; `mod.rs` holds the module's
  description, its error type and the re-exports. The re-export keeps the paths
  `crate::workspace::WorkspaceManager` and so on as they are.
- **What moves:**
  1. `workspace/mod.rs` → `workspace/manager.rs`; the description and `WorkspaceError` stay.
  2. `controller/mod.rs` → `controller/manager.rs`; the description, which explains all
     controllers, stays.
  3. `group.rs` → `group/mod.rs` and `group/manager.rs`.
  4. `agent` as `session`: `mod manager` private with a `pub use`, `AgentError` in `agent/mod.rs`;
     the eight imports of `agent::manager::…` become `agent::…`.
  5. In the model, `user_provider/manager.rs` for the `UserManager`.
- **What does not:** `SecretManager` - `secret/mod.rs` has 164 lines and little but the manager -,
  and `SecurityManager`, which manages nothing and is renamed instead
  ([365](365-security-manager-is-an-access-policy.md)).
- **Not here:** `runtime.rs` (847 lines) holds `RuntimeManager`, `ChannelRuntime`, `ActorRuntime`
  and `RuntimeBuilder`; splitting it into a module `runtime/` is
  [366](366-runtime-rs-holds-four-types.md), whose `manager.rs` follows the rule of this issue.
- Moves only, no behaviour changes; the moved halves through `git mv`, so that their history
  stays readable.

## Done when

- Every manager of a kind lives in its module's `manager.rs`, exported from `mod.rs`, and the rule
  is in CODING-GUIDELINES.md.
- No behaviour changes; clippy and the tests pass.

## History

- **2026-10-15** — Opened at the maintainer's request, under epic [349](349-core-review-epic.md):
  before [354](354-names-follow-several-schemes.md), which then finds the files where they belong.
