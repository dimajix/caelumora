# 452 — Rules of the guidelines that a lint could check are only written down

**Status:** in progress
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** Most rules of CODING-GUIDELINES.md that clippy could check - imports, no `unwrap`, `pub(crate)` by default, `expect` instead of `allow`, no `println!` - are only written down, and are missed; nothing notices when a function grows too complex.

## The problem

`[workspace.lints]` turns on one lint, `clone_on_ref_ptr`. Every other rule of
`docs/CODING-GUIDELINES.md` is kept only as far as whoever writes the code remembers it, and the
reviews show that rules are missed - the import rule most of all. A rule a tool checks is kept; a
rule in prose is missed sooner or later (`AGENTS.md`, "Corrections become rules").

Counted on `main` at 2529d3b9, with `allow-unwrap-in-tests`, `allow-expect-in-tests` and
`allow-print-in-tests`:

| Lint | Rule | Found |
|---|---|---|
| `clippy::absolute_paths` | [Imports](../docs/CODING-GUIDELINES.md#imports) | 513, of them 325 outside tests; mostly `tokio::time::sleep`, `std::mem::take`, `axum::middleware::from_fn`; 13 `crate::…` |
| `clippy::unwrap_used`, `clippy::expect_used` | no `unwrap()`/`expect()` in production code | 5 in production code (`caelumora-model/src/digest.rs`, 3 in `caelumora-runtime`); about 200 in `caelumora-model/src/store/suite.rs`, a test suite in `src/` |
| `unreachable_pub` | visible only what is used from outside | 128 |
| `clippy::allow_attributes`, `clippy::allow_attributes_without_reason` | `expect` rather than `allow`, with a reason | 6 |
| `clippy::print_stdout`, `clippy::print_stderr` | never `println!` | 54, all in `caelumora-tui` |
| `clippy::dbg_macro`, `clippy::todo` | - | 0 |
| `clippy::cognitive_complexity` | - | 28 functions |
| `clippy::too_many_lines` | - | 5 functions |

## Considerations

- **Imports.** With `absolute-paths-max-segments = 2` in `clippy.toml`, the lint accepts a function
  called by its module (`time::sleep` after `use tokio::time;`), which the rule allows - decided
  with the maintainer on 2026-10-23 -, and refuses every longer path. A path an import cannot serve
  (two items of one name, `thiserror::Error`) has two segments or is renamed on import.
- **Tests.** `allow-unwrap-in-tests` and its siblings cover `#[test]` functions and `#[cfg(test)]`
  modules, not test code compiled for other crates: `store/suite.rs` and the `testing` modules need
  an `expect` of their own, on the module, with its reason.
- **The TUI writes to the terminal.** Its output is meant: the place that prints carries an
  `#[expect(clippy::print_stdout, reason = "…")]`, so that printing stays confined to it.
- **Complexity without a hard limit.** `cognitive_complexity` (a nursery lint) and
  `too_many_lines` are turned on, and every function they find today gets
  `#[expect(…, reason = "issue 452")]`. A new function over the limit is refused by the pre-commit
  hook; a function made simpler makes its `expect` fire as unfulfilled, and the attribute is
  removed. The attributes are the list of what is still to be simplified, and it can only get
  shorter. The nursery lint's measure may change with clippy; a change shows as an unfulfilled
  `expect` or a new warning, both visible.
- **Rules a lint cannot check** stay in the checklist of CODING-GUIDELINES.md; the guidelines name
  which lint checks which rule, so that nobody writes a rule down twice.

## Steps

1. **The cheap lints.** `dbg_macro`, `todo`, `allow_attributes`, `allow_attributes_without_reason`,
   `print_stdout`, `print_stderr`, `unwrap_used` and `expect_used` in `[workspace.lints]`, a
   `clippy.toml` allowing them in tests, and the places found fixed or given a reasoned `expect`.
2. **`unreachable_pub`.** Every item made visible only as far as it is used.
3. **Complexity.** `cognitive_complexity` and `too_many_lines`, the functions found today marked
   with `expect`.
4. **`absolute_paths`.** With `absolute-paths-max-segments = 2`; every longer path imported.
5. **Line lengths.** `scripts/line-length.sh` in the pre-commit hook, on the lines a commit
   changes; the lines too long today - 75 in Markdown, 18 in Rust comments - wrapped once.

Done when the lints are on, clippy is silent, and CODING-GUIDELINES.md says which rules a lint
checks.

## History

- **2026-10-23** — Issue created while reworking `AGENTS.md`, after counting what these lints find
  in the code today.
- **2026-10-23** — Step 5 added: the line lengths are counted by `scripts/line-length.sh` since
  the reviewer agent came, and the hook can run it on what a commit changes.
- **2026-10-23** — Step 1: the cheap lints are on. Test code outside `#[cfg(test)]` - the store
  suite, the `testing` modules, `AgentContextView::of_state`, the helpers of four integration
  tests - says so with a module-wide `expect`; the TUI's three printing functions and the build
  script's `fail` with one of their own. In production code, `digest::to_json` keeps its
  `expect`, marked: the model types hashed always serialise, and the digest must not go on with
  `Null` - an exception the maintainer accepted. Turned into `expect`, three `allow`s turned out
  to suppress nothing any more and were removed (`ArtifactManager::new`, `HistoryEntry`,
  `TranscriptEntry`).
- **2026-10-23** — Step 2: `unreachable_pub` is on, as a lint of `[workspace.lints.rust]`; its 128
  places - most of them the handlers of the REST routes - are `pub(crate)` or `pub(super)` now,
  as the lint proposed, ten constants used only in their own file private. The lint finds only
  the `pub` items a crate's root cannot reach; a `pub` item in a public module that only its own
  crate uses passes it (`routes::ensure_session`, say), and is left for its owner to find.
  Seven of the modules it touched have no sentence of their module docs; decided with the
  maintainer that a mechanical change does not count as changing a module, and the rule in
  CODING-GUIDELINES.md says so now.
