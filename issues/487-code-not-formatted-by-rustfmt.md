# 487 — The Rust code is not formatted by rustfmt, and nothing checks how it is laid out

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `cargo fmt --check` differs in 329 of the workspace's Rust files, lines of code run up to more than 500 characters, and no rule, hook or check says how Rust code is laid out, although `scripts/wrapping.sh` leaves code out of its count on the assumption that rustfmt keeps it.

## The problem

Nothing keeps the layout of the Rust code. `cargo fmt --check` (rustfmt 1.10.0, without a
`rustfmt.toml`) differs in 329 files of every crate, in about 6900 places: imports not sorted or
merged, signatures and calls not broken, blocks laid out by hand. 11138 lines of code are longer
than 100 characters, 6215 longer than 120, the longest more than 500. A wider `max_width` does not
make the difference small: with 120, rustfmt still differs in about 6200 places.

No document names rustfmt as a rule, the pre-commit hook does not run it, and the reviewer does not
look for it. Only `scripts/wrapping.sh` mentions it: it counts only the comments of a Rust file,
"since rustfmt keeps the code" - which today nothing does. Found in the work on issue
[484](484-a-dash-at-line-start-is-checked-only-in-rust-docs.md).

## Why it matters

- A diff mixes what was changed with how an editor or an agent happened to lay out the lines
  around it; formatting by hand costs time in every change, and a reviewer reads the noise too.
- Lines of several hundred characters cannot be read side by side in a diff or a review.
- A rule that nothing checks is missed ([AGENTS.md](../AGENTS.md#corrections-become-rules)); here
  there is not even the rule.

## Considerations

- **Which configuration.** rustfmt's defaults - `max_width = 100`, the width at which comments and
  documents are wrapped - or a `rustfmt.toml` with a wider `max_width` or other settings. Settings
  that only nightly rustfmt knows (`imports_granularity`, `group_imports`, `wrap_comments`) would
  tie the check to a nightly toolchain; stable settings alone keep it on the toolchain the workspace
  builds with.
- **One commit that formats everything**, with nothing else in it, named in a
  `.git-blame-ignore-revs` so that `git blame` passes over it. It touches nearly every Rust file:
  every branch open at that moment conflicts with it, so it is done when none is open, or each open
  branch is formatted itself before it is merged.
- **Keeping it.** The pre-commit hook runs `cargo fmt --check` whenever Rust code is part of a
  commit, as it runs clippy, and refuses the commit otherwise; or it formats the staged files
  itself, which changes files behind the author's back and mixes with what is not staged. A check
  that refuses is what the hook does for clippy and for wrapping.
- **The documents.** CODING-GUIDELINES.md names the rule beside the wrapping of comments, and
  `scripts/wrapping.sh` keeps leaving code to rustfmt, now rightly.

## Done when

- The guideline says how Rust code is laid out, `cargo fmt --check` passes on the whole workspace,
  the pre-commit hook refuses a commit of Rust code it does not pass, and the commit that formatted
  the code is ignored by `git blame`.

## History

- **2026-10-28** — Opened at the maintainer's request, from the findings of issue 484.
