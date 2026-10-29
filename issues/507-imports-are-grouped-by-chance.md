# 507 — Imports are grouped by chance

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Nothing says how the imports of a module are grouped, and neither rustfmt nor a check keeps it, so about 20 modules split their imports by blank lines into groups of no common order, and a script or `cargo clippy --fix` puts a new import into whichever group comes first.

## The problem

Most modules import in one block, which rustfmt sorts - `super`, `crate`, then the other crates
and `std` by name -, and keep their re-exports (`pub use`) in a block of their own. About 20
import blocks in 20 files split their private imports by blank lines into two to four groups,
without an order between them: a group of `crate`, `caelumora_model` and `std` imports, then
another of the same kinds (`caelumora-runtime/src/channel/participants.rs`,
`channel/settings.rs`, `session/channels.rs`, `view/mod.rs`, `tests/security_suite.rs`,
`caelumora-agents/src/tool/mod.rs`, among others). Only one of them groups by origin
(`caelumora-mcp/tests/mcp_tool_integration.rs`).

rustfmt sorts within a group and leaves the groups as they are. The options that would group by
origin or merge imports (`group_imports`, `imports_granularity`) are unstable, and the workspace
builds with a stable toolchain. The section *Imports* of the coding guidelines says how a path is
written, not how imports are grouped.

## Why it matters

A reader finds an import in a group by chance, and the same module path may stand in two groups.
Issues [501](501-tests-build-a-personality-field-by-field.md) and
[505](505-tests-build-sessions-and-users-field-by-field.md) rewrote imports by script; in these
modules the new imports landed in the first group, apart from their siblings, and had to be moved
by hand - which the next mechanical change has to do again.

## Considerations

- **The rule.** Private imports in one block, sorted by rustfmt; re-exports (`pub use`) in a block
  of their own after them. The alternative, groups by origin (std, other crates, the crate's own),
  is what `group_imports = "StdExternalCrate"` would keep on nightly, but almost no module follows
  it today, and on stable nothing would enforce it.
- **The check.** As `scripts/tuple-bindings.sh` checks the length of tuple bindings: a script that
  finds a blank line within the private imports of a module, called by the pre-commit hook on the
  Rust files a commit adds or changes. A module whose imports are grouped deliberately would need
  an exception the script knows; none is known today.
- **The change.** The rule and the check first; then the 20 blocks merged in a mechanical change of
  its own, which `cargo fmt` sorts.

## Done when

- The coding guidelines say how imports are grouped, a check of the pre-commit hook keeps it, and
  no module breaks it.

## History

- **2026-10-29** — Opened from the findings of the reviews of issues
  [501](501-tests-build-a-personality-field-by-field.md) and
  [505](505-tests-build-sessions-and-users-field-by-field.md).
