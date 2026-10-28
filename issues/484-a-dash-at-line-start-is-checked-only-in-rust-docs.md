# 484 — A comment line that starts with a dash is found only in Rust's doc comments

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** "No line of prose starts with a dash" is checked by clippy in Rust's doc comments alone; in plain Rust comments and in the web UI's TypeScript it is kept by hand, and missed.

## The problem

[CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md) asks that no line of prose start with a dash:
a sentence wrapped so that a line begins with `- ` reads, to rustdoc and to Markdown, as a list
item, and breaks apart. Clippy's `doc_lazy_continuation` finds it in Rust's doc comments (`///`,
`//!`). Nothing finds it elsewhere:

- in plain Rust comments (`//`), where `caelumora-model/src/macros.rs:12` and
  `caelumora-model/src/security/ownership.rs:105` begin with `- ` today;
- in the comments of the web UI's TypeScript, whose width `scripts/line-length.sh` checks since
  issue [479](479-a-rename-refused-goes-to-the-banner.md), and where the rewrapping of that issue
  made two such lines, found only by hand.

## Why it matters

A rule missed twice is checked by a tool, not written down more emphatically
([AGENTS.md](../AGENTS.md#corrections-become-rules)). Rewrapping - by hand or by a script - makes
such lines easily, since a ` - ` that sets off a clause is where a line is broken as much as any
space.

## Considerations

- **A. `scripts/line-length.sh` finds them too**, in the comments it checks already - Rust's and the
  web UI's -, where a line after the comment marker begins with `- ` and the line before it is
  prose of the same comment, not a list. It runs in the pre-commit hook, so a commit with one is
  refused. The script then checks how prose is wrapped, not only how wide; its name and the
  guideline would say so.
- **B. A check of its own** beside `line-length.sh`, in the hook.
- **Whether the rule holds for plain comments at all.** The guideline names doc comments; in a plain
  comment, and in TypeScript, nothing renders the dash as a list, but the reader still reads it as
  one. Before a tool, the guideline says where the rule holds.

## Done when

- The guideline says where the rule holds, a tool finds every line that breaks it there, and the
  lines that break it today are rewrapped.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 1 of issue 479.
- **2026-10-28** — Done. The rule holds for all prose the wrapping rules cover - Markdown documents,
  Rust's comments of every kind and the web UI's TypeScript comments - as the maintainer decided.
  `scripts/line-length.sh` checks it and is now `scripts/wrapping.sh` (option A). A dash goes on
  with a sentence where it stands where the prose of the line before begins - after its marker, in
  a list item -, and that line belongs to the same comment, is no heading or table row, and ends in
  none of `.`, `:`, `?`, `!` or `**`; a dash where the item before has its marker is the next item.
  Clippy's `doc_lazy_continuation` turned out to miss a dash in a doc comment where it ends its
  paragraph, and nothing looked inside list items: 41 lines broke the rule, not 2 - 10 in Rust, 8
  of them in doc comments, 2 in `docs/` outside lists and 2 inside them, the rest in issues,
  resolved ones included. Their paragraphs were rewrapped without changing a word, so that the hook
  does not refuse the next commit of them; in issue 169, a nested list introduced by "if" got the
  colon that tells it from a sentence going on.
