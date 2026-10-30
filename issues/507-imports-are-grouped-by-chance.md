# 507 — Imports are grouped by chance

**Status:** done
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

## Decision

One block, the re-exports before it: a module's `mod` declarations, then its re-exports
(`pub use`) as a block of their own, then its private imports in one block without blank lines -
in a `mod tests` as at the top of a file. Grouping by origin is the more common convention (the
Rust project's own `rustfmt.toml`, rust-analyzer's style guide), but on stable nothing keeps it, a
check would have to sort every path by origin, and an import added by a script would still land in
the wrong group; the Rust style guide leaves the groups to the author. The re-exports stand before
the imports, as in most modules that have them, next to the `mod` declarations they export;
rust-analyzer puts them after, as items. The check is `scripts/import-blocks.sh`, which reads a
module from the indentation rustfmt gives it, and checks the `mod` declarations too, so that the
rule it is named for is the rule it checks.

Two steps: the rule and the check; then every module the check finds put in order - its blocks
merged, its re-exports and `mod` declarations moved before its imports.

## Done when

- The coding guidelines say how imports are grouped, a check of the pre-commit hook keeps it, and
  no module breaks it.

## History

- **2026-10-29** — Opened from the findings of the reviews of issues
  [501](501-tests-build-a-personality-field-by-field.md) and
  [505](505-tests-build-sessions-and-users-field-by-field.md).
- **2026-10-30** — Decided: one block, the re-exports before it; on branch
  `issue-507-import-blocks`. Step 1: the rule in CODING-GUIDELINES.md, `scripts/import-blocks.sh`
  and its call in the pre-commit hook. The script finds 37 files out of order: 30 split blocks in
  21 files, re-exports after the imports in 11, and `mod` declarations after the imports in 14 -
  among them a `mod tests;` at the bottom of `caelumora-store-postgres/src/lib.rs`.
- **2026-10-30** — Done: step 2 put the 37 modules in order, by a script that rearranged each run
  of declarations and imports and by hand where an item stood between them - the re-exports of
  `terminal_tool.rs` after a constant, the `mod` declarations of `caelumora-workspace-tools`'s
  root after its imports, and the `mod tests;` at the bottom of `caelumora-store-postgres`'s.
  `scripts/import-blocks.sh` finds none left.
