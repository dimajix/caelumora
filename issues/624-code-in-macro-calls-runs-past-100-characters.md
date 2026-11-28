# 624 — Code in macro calls runs past 100 characters

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** rustfmt leaves the inside of a macro call it cannot parse as it is written, and nothing checks the rule that such code is broken by hand within 100 characters: about 170 lines of Rust code without a string literal are longer, most of them in `matches!` with a guard, and a change that touches one keeps it long unnoticed.

## The problem

CODING-GUIDELINES.md (*Comments and documentation*, issue
[487](487-code-not-formatted-by-rustfmt.md)) lays code out by rustfmt, which keeps it within 100
characters - but for two things it leaves as written: comments, and the inside of a macro call it
cannot parse (`matches!` with a guard, `tokio::select!`, a `macro_rules!` of our own), which "is
broken by hand within 100 characters as rustfmt would break it". `scripts/wrapping.sh` checks the
comments; its header says of the macro calls that nothing checks them.

On 2026-11-27, 171 lines of Rust code outside comments and without a quote were longer than 100
characters, up to 244:

- 86 in `matches!`, most of them in tests (`caelumora-kernel/tests/security_suite.rs`,
  `caelumora-kernel/src/channel/participants.rs`);
- 7 in `context!` of the prompt catalogue (`caelumora-notes/src/component/notes.rs:438`,
  `caelumora-memory/src/component/mod.rs:117`, `caelumora-skills/src/component/prompt.rs:23`, ...);
- some in `yield` of `async_stream`, in `tokio::select!` and in the macros of
  `caelumora-store/src/suite.rs`;
- 23 the head of a test whose name alone leaves no room - which no layout shortens, and which the
  rule does not mean;
- 8 inside a string literal continued with `\`, which may be longer.

The reviewer of step 5 of issue [620](620-dependencies-go-unused-and-out-of-date-unnoticed.md)
found two of them in lines that step had changed, and six more beside them.

## Why it matters

- The rule has been missed many times; by AGENTS.md (*Corrections become rules*), a rule missed
  twice is checked by a tool, not written down more emphatically.
- A line of 160 or 240 characters is read by scrolling sideways, in a review above all.
- A change that touches such a line keeps it as it was: nothing tells its author.

## Considerations

- **A. A check of its own in the pre-commit hook** (`scripts/code-width.sh`, say): Rust lines
  outside comments longer than 100 characters, but for a line with a string literal - or inside
  one - and a line that cannot be broken, a single word past its indent. Like `wrapping.sh`, it
  checks the lines a change adds or changes, and whole files when it is given them. Recommended.
- **B. `wrapping.sh` checks code too.** One script fewer, but it is about prose, and its rules - a
  dash going on with a sentence, a link not broken - are no rules of code.
- **C. rustfmt's `error_on_line_overflow`.** It reports what rustfmt leaves too long, but is
  unstable: it needs a nightly toolchain, which the workspace does not use.

Apart from the check: the lines long now are broken once, in a step of their own - else every
change touching one of them has to break it first -, or as they are touched.

## Done when

- A commit that adds or changes a line of Rust code longer than 100 characters - outside a comment,
  a string literal or a word that cannot be broken - is refused by the pre-commit hook.
- The lines long now are broken, or the decision to break them as they are touched is recorded.
- CODING-GUIDELINES.md names the check where it states the rule.

## Decision

Decided by the maintainer on 2026-11-28, on what was found in trying rustfmt by hand:

- **rustfmt itself lays out lines longer than 100 characters**, and takes back a line broken by
  hand there: the head of a test with a long name, a return type (`caelumora-compaction`'s
  `napping_with`), the pattern of a `let … else`, `async move { … }` on one line, even an
  `assert!` of 101 characters. On 2026-11-28 there were 32 of them - not the 23 heads of tests
  alone. A check of every line, as option A has it, would refuse what `cargo fmt --check` asks
  for.
- **What rustfmt leaves as written is not only the inside of a macro call.** It also leaves a
  whole statement that holds a string literal it cannot fit (`KernelBuilder` in the tests of
  `kernel/builder.rs`, `From<ArtifactError> for ToolError`), while it lays out the arguments of
  an `assert!` itself. Whether a line lies in a macro call does not tell the two apart.

**The check asks rustfmt.** `scripts/code-width.sh` takes every line of Rust code longer than 100
characters - but for a comment, a line with a string literal or inside one, and a single word past
its indent -, breaks it at every space in a copy of the file, and lets rustfmt lay out the copy: a
line rustfmt restores is its own, a line it leaves broken is the author's, and is reported. On
2026-11-28 that found 128 lines in 49 files, as every line tried by hand had shown.

**The long lines are broken once**, in a step of their own: the pre-commit hook gives a check whole
files, so a commit would otherwise have to break every long line of each file it touches. Without
files, the script checks every Rust file of the repository, as `scripts/tuple-bindings.sh` and
`scripts/import-blocks.sh` do, since none is to be found.

**How a file is read as Rust text is shared**: `scripts/rust-text.awk`, loaded by
`tuple-bindings.sh` and `code-width.sh`.

## Plan

Each step a commit of its own, reviewed before the next.

1. **The reading shared.** `scripts/rust-text.awk` - a file read as one text, `literal`, the
   reading of each file before its `check` - out of `scripts/tuple-bindings.sh`, without a change
   of behaviour.
2. **The long lines broken**, by hand as rustfmt would break them; `cargo fmt --all --check`
   passes.
3. **The check.** `scripts/code-width.sh` and the pre-commit hook; CODING-GUIDELINES.md corrected -
   rustfmt's own long lines stay, what it leaves as written is broken by hand - and the check
   named there and in its checklist; the header of `scripts/wrapping.sh`.

## History

- **2026-11-27** — Opened on the maintainer's request, from the review of step 5 of issue 620.
- **2026-11-28** — Decision and plan recorded; step 1: how a file is read as Rust text moves to
  `scripts/rust-text.awk`, which `scripts/tuple-bindings.sh` loads.
- **2026-11-28** — Step 2: the 128 lines broken, in 49 files. A `matches!` with a guard and the
  body of a `stream!` were laid out by rustfmt itself, masked in a copy as a call it can parse
  (`matchez!`, the guard's `if` as `||`; `stream!({ … })`) and unmasked again, taking only what
  touched a long line; the rest by hand. Where a `matches!` broken within 100 characters let
  rustfmt lay out the statement around it, `cargo fmt` did. Two functions grew past 100 lines -
  `names_are_unique` of the store's suite and a test of `channel/participants.rs` - and carry
  `#[expect(clippy::too_many_lines)]` naming this issue, as the maintainer decided;
  CODING-GUIDELINES.md names it beside issue 490.
- **2026-11-28** — Added to step 2: every `matches!` with a guard laid out as rustfmt lays out a
  call, the call around it too - those longer than 100 characters because of a string literal,
  those left in the layout rustfmt falls back to around a macro it cannot parse, and the older
  layouts by hand, which no check finds. A binding `ref x` was masked as `&&& x`; in a field of a
  pattern written short (`{ ref name, .. }`) that is no expression either, and those six were
  laid out by hand. In `let f = |e| { matches!(…) }` rustfmt takes the block away again while the
  `matches!` stands on one line, and joins it to one line longer than 100; there the `matches!`
  is broken within the block.
  The chain of `.bind` in `caelumora-store-postgres/src/histories.rs`, which rustfmt left as
  written for the SQL string before it, laid out as the one above it.
- **2026-11-28** — Step 3, the check: `scripts/code-width.sh` asks rustfmt whose a line longer
  than 100 characters is, as decided; a space before a comment that ends the line is no place to
  break at, since a break there moves the comment to a line of its own, which rustfmt keeps. The
  pre-commit hook gives it the Rust files a commit adds or changes. CODING-GUIDELINES.md says
  which long lines are rustfmt's and stay, which three things it leaves as written, how they are
  broken, and names the check; the header of `scripts/wrapping.sh` points to it. Run on the files
  as they were before step 2, it finds the same 128 lines; on the repository, none.
  `scripts/tuple-bindings.sh` finds `scripts/rust-text.awk` by its own place before it changes to
  the top of the repository, which a call by a relative path from below it had missed since
  step 1. Done.
