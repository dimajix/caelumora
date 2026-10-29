# 489 — Values are bound by a tuple made on the spot, in 426 places, and nothing checks it

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** About 426 `let` lines in 119 files bind independent values by a tuple made only to be taken apart again - `let (alice, bob) = (UserId::new(), UserId::new());` - against the rule of CODING-GUIDELINES.md, and no lint, hook or check finds them.

## The problem

Two values are bound by two `let` lines, never by a tuple made to be taken apart again
([CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md#small-return-values), written down in issue
[485](485-own-project-derived-in-several-places.md)). The rule had been given in a review before,
on `Agent::security_context` (issue 341), and was missed again in the work on 485 - a rule missed
twice is checked by a tool (AGENTS.md, "Corrections become rules").

The code does not keep it. A grep for `let (…) = (…)` on `main` finds about 426 lines in 119
files, in nearly every crate - 157 in `caelumora-runtime`, 126 in `caelumora-model`, 66 in
`caelumora-agents`, 24 in `caelumora-artifacts` -, in tests and in production code:

- values made at once: `let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());`
- values read from elsewhere: `let (agent_id, name) = (self.agent_id(), self.agent.name());`
- fields of one struct: `let (request, retries) = (taken.request, taken.retries);` - which is
  better taken apart as the struct it is, `let Taken { request, retries } = taken;`.

Clippy has no lint for it, and neither the pre-commit hook nor the reviewer looks for it, so every
new line copied from an old one repeats it.

## Why it matters

- Two values written as one read as if they belonged together, and a long line of them hides
  which value goes to which name.
- As long as the old lines stand, they are the pattern new code is copied from: the line the rule
  was missed on in issue 485 was copied from the test beside it.

## Considerations

- **A. Sweep, then check.** Rewrite all of them in one commit of its own, without any other
  change, then add a check to the pre-commit hook - a script beside `scripts/wrapping.sh`, run on
  the Rust files a commit adds or changes - that refuses `let (…) = (…);` whose right side is a
  tuple of as many parts. The sweep is mechanical but large; it touches lines that issue
  [487](487-code-not-formatted-by-rustfmt.md) (rustfmt) touches too, so the two are best done one
  right after the other, whichever first.
- **B. Check only what a commit adds.** The hook refuses the pattern in the lines a commit adds,
  and the old ones go as their files are worked on. No large commit, but the old lines stay for
  a long time as the pattern to copy, and a check on added lines alone is a second way of checking
  next to `scripts/wrapping.sh`, which reads whole files.
- **C. Sweep without a check**, and let the reviewer look for it. Nothing keeps it after the
  sweep - the state this issue is about.

Not every such line is a case of the rule: a swap, `(a, b) = (b, a)`, needs the tuple, and an
assignment that must happen only once every value was made - `Session::describe` assigns its
summary and briefing together, so that a failure leaves both unchanged - is no binding. The check
has to let such lines pass, or name its exceptions.

Recommendation: A.

## Done when

- No `let` binds independent values by a tuple made on the spot, and the pre-commit hook refuses a
  new one.

## History

- **2026-10-29** — Opened at the maintainer's request from the findings of issue 485.
