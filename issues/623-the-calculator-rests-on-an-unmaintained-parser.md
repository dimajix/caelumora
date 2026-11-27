# 623 — The calculator rests on an unmaintained parser

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Maintenance
**Summary:** The tool `calculate` of the component `calculator` evaluates expressions with `meval` 0.2.0, which has no newer version and pulls in `nom` 1.2.4, whose code Cargo warns a future version of Rust will reject, so the calculator stops building with a coming compiler and no update can help.

## The problem

`calculate` (`caelumora-agents/src/tool/calculator.rs`, component `calculator`) hands the model's
expression to `meval::eval_str`. Its description promises expressions such as `2 + 2`,
`sin(pi/2)` and `exp(1)` (`caelumora-agents/src/prompts.yaml`, `calculator`).

- `meval` 0.2.0 is its newest version; it is not maintained any more.
- It depends on `nom` 1.2.4. Every build reports: "the following packages contain code that will
  be rejected by a future version of Rust: nom v1.2.4" (`cargo report future-incompatibilities`).
- Nothing else of the workspace depends on `nom` 1.

Found while bringing the dependencies up to date (issue
[620](620-dependencies-go-unused-and-out-of-date-unnoticed.md), step 2): an update cannot help, only
replacing the parser.

## Why it matters

- Once a release of Rust turns the warning into an error, `caelumora-agents` no longer builds, and
  with it no program.
- A warning in every build that nobody acts on teaches to overlook warnings.

## Considerations

- **A. Another crate that evaluates expressions**, maintained, taking what `meval` takes: the four
  operations, powers, parentheses, `pi` and `e`, the usual functions (`sin`, `cos`, `exp`, `ln`,
  `sqrt`, ...). `cargo search` names `nanoexpr` among others; not examined yet. What to compare:
  whether it is maintained, its dependencies, the functions and constants it knows, how it fails on
  input it does not take, and that it evaluates without running anything else.
- **B. A small evaluator of our own** - a parser of a few dozen lines for what the description
  promises. No dependency, but code of ours to keep, for what crates do already.
- **C. The calculator removed** - models compute simple things themselves, and an agent with a
  workspace can run a command for anything more. But it is a tool without a workspace, and models
  are unreliable at arithmetic.

Recommended: A, the crate chosen by the comparison above; B only if none fits.

## Done when

- `calculate` evaluates what its description promises without `meval`, and no build reports `nom`
  1.2.4; its tests hold, with a case for each kind of expression the description names.

## History

- **2026-11-27** — Opened on the maintainer's request, from step 2 of issue 620.
