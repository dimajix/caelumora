# 454 — The warnings of rustdoc are checked by nothing

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `cargo doc --workspace` warns 31 times that public documentation links to a private item, and neither clippy nor the pre-commit hook runs rustdoc, so such warnings only grow.

## The problem

`cargo doc --workspace --no-deps` warns 31 times, all of one kind: `public documentation for … links
to private item …` (rustdoc's `private_intra_doc_links`) - 26 in `caelumora-runtime`, 4 in
`caelumora-skills`, 1 in `caelumora-rest-server`. The pre-commit hook runs clippy, which does not
run rustdoc's lints, so nothing notices a new one; found while working on step 2 of
[452](452-guideline-rules-not-checked-by-lints.md), where the count stayed the same before and
after.

A link from public documentation to a private item leads nowhere in the rendered documentation:
the reader of the public API is sent to something they cannot see. Either the link names what
the public API offers instead, or the text names the private item without linking it, or the
documentation describes something that belongs on the private item's own docs.

## Considerations

- **Fixing the 31.** Each is judged by itself, by the three ways above; most are probably
  doc comments that describe how a public method works inside, which belongs in the private
  item's docs (`docs/CODING-GUIDELINES.md`, "What goes where").
- **Keeping them at zero.** `[workspace.lints.rustdoc]` can set `private_intra_doc_links` and the
  other rustdoc lints to `warn`, but only a run of rustdoc reports them. The pre-commit hook could
  run `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"` whenever Rust code is
  committed - it costs the time of a documentation build on top of clippy's. Running it only
  for the crates a commit touches would be cheaper, but a link can break in a crate the commit
  does not touch.
- **Not the same as clippy's `doc_*` lints**, which clippy already runs and the hook already
  checks.

Done when `cargo doc --workspace --no-deps` is silent and a commit that makes it warn is refused.

## History

- **2026-10-23** — Issue created during step 2 of
  [452](452-guideline-rules-not-checked-by-lints.md), after counting the warnings of rustdoc
  before and after it.
