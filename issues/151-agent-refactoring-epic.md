# 151 — Refactoring epic: the review of the agent actor of 2026-09-30

**Status:** done
**Area:** Maintenance

## The problem

A review of `AgentActor` and what it works with - `Turn`, `ToolCalls`,
`AgentNotes`, `AgentMemory`, `AgentStateHandle` and the model types behind
them (`work/2026-09-30-code-review-agent.md`) - looked for what can be
simpler: complex logic, helper types, the amount of code, and what belongs
in a place of its own. The cut of [83](83-turn-logic-on-agent-actor.md),
[84](84-agent-state-machine.md) and [85](85-turn-state-in-agent-channel-state.md)
holds; what is left are four spots of real complexity and a number of
small duplicates. Each group of findings is its own issue, 152-160, with
[92](92-ledger-for-notes-and-memory.md) from the earlier review taken in.
This issue is the bracket.

## Why it matters

`actor_impl/agent.rs` has 1100 lines of production code, and the end of a
turn - the part issues 141, 145, 146 and 148 keep changing - is spread over
three signals and three paths. The findings interlock: the give-up paths
(153) change with the signals (152), the notes' fold (156) is the core of
the ledger (92), and splitting the file (159) only pays once the rest has
moved.

## The goals and where they conflict

The review weighed four goals: simpler logic (A), fewer helper types (B),
less code (C), functionality in places of its own (D).

- 152 and 155 each add an enum (against B) but remove more than they cost:
  two fields, a CAS loop and a precedence rule; knowledge of
  `ToolCallState` in the actor.
- D is met by files (159), not by new types, where no type is missing.
- 156 and 92 are worth more together than apart.

## Recommended order

1. [160](160-agent-small-cleanups.md) - hygiene and dead surface, no risk.
2. [152](152-three-signals-end-a-turn.md), then
   [153](153-three-give-up-paths.md) - the run loop; the channel tests
   (cancel and stop while waiting for approval, lowering, raising) are the
   net.
3. [154](154-model-step-mixes-four-things.md).
4. [155](155-approval-admission-on-the-actor.md).
5. [92](92-ledger-for-notes-and-memory.md), then
   [156](156-notes-and-memory-staged-scans.md).
6. [157](157-actor-holds-what-is-not-its-own.md),
   [158](158-publish-question-built-five-times.md).
7. [159](159-split-agent-actor-file.md), if still wanted.

## Considered and not taken up

- **A `Turn` that exists only while a turn runs** (review T4): cleaner
  typing - no `NO_TURN`, no `debug_assert!` in `current_mut` - but the
  `MessageProcessor` trait would split into one on the state and one on
  the turn, and about 25 tests change. Not now: the convention costs
  nothing while only the two appenders work between turns.
- **`EndedTurn` and `CommittedTurn` as one type** (T1): one type less, but
  with fields half of its users do not mean. Decision (B) of 85 stands.
- **`Transcript::push_entry` taking the digest of a `TurnEntry` over**
  (M2): small gain, same nesting; not urgent.

## Done when

Every issue of the order above is `done` or `rejected`, and this one
records what changed in the order along the way.

## History

- **2026-09-30** — Created from the review
  (`work/2026-09-30-code-review-agent.md`) at the maintainer's request.
- **2026-10-01** — [156](156-notes-and-memory-staged-scans.md) done, after
  [92](92-ledger-for-notes-and-memory.md) as planned.
- **2026-10-01** — [159](159-split-agent-actor-file.md) done without
  splitting the file: what needs no actor became free functions, the run
  control a type of its own (`RunControl`), and the tests of other modules
  moved to them. Every issue of the order is done; the epic is closed.
