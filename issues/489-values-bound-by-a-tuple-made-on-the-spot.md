# 489 — Values are bound by a tuple made on the spot, in 426 places, and nothing checks it

**Status:** done
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
- **2026-10-29** — The rule narrowed by the maintainer, after a first sweep of all 423 lines read
  worse than before: a tuple may bind values that belong together - of one kind in one role, the
  parts of one thing, what is cloned into a closure together - as long as the line stays within
  100 characters; values of different kinds that merely begin at the same place get a `let` each,
  as does every binding that does not fit on one line. Whether values belong together is a
  judgement no script makes - a test of "the same constructor on every side" marked
  `(OthersWords::Wait, OthersWords::Hear)` as well -, so the check (step 2) keeps the length
  alone, and the rest is the reviewer's, by the checklist. Step 1, the sweep: the 87 bindings
  longer than 100 characters are written as one `let` each, a pair that belongs together kept as
  a tuple beside them; of the shorter ones, 12 that bind values of different kinds are
  taken apart. `view/channel.rs` named a closure `new` and bound `new` by the same tuple: it is
  now `heard_from_others`. From the review, the maintainer had 9 more taken apart - among them a
  search's matches, counts and flag in `workspace-tools/src/files.rs`, and the fields of an
  `ArtifactSnapshot`, now taken apart as the struct it is; the pairs that are parts of one thing
  (a session's id and owner), actors, loop state advanced together, and what is filled and
  returned together stay.
- **2026-10-29** — Step 2, the check: `scripts/tuple-bindings.sh` finds a tuple binding longer
  than 100 characters or spread over several lines - so that it still finds one once rustfmt
  (issue 487) breaks it, stepping over comments and string literals -, in every Rust file or in
  those it is given, and leaves `let … else` and a tuple of one alone; the pre-commit hook gives
  it the Rust files a commit adds or changes. Run on `main` before step 1, it found the 87
  bindings the sweep took apart, and none after it. CODING-GUIDELINES.md names it beside the
  rule. Whether values belong together stays the reviewer's.
