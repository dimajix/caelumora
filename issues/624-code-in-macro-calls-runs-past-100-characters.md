# 624 — Code in macro calls runs past 100 characters

**Status:** open
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

## History

- **2026-11-27** — Opened on the maintainer's request, from the review of step 5 of issue 620.
