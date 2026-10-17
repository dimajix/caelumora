# 156 — Notes and memory search a turn's staged changes by hand

**Status:** done
**Area:** Context and knowledge
**Epic:** [151](151-agent-refactoring-epic.md)
**Summary:** Notes and memory answer "the chain with this turn's changes" with eight hand-written scans instead of the fold the chains already have.

## The problem

Every operation of `AgentNotes` and `AgentMemory` asks the same question - what the chain looks like
with this turn's changes in it - and answers it with its own scans over the staged changes: `note`
looks for notes noted now, `revoke` for a staged note to drop and for a staged revocation,
`reclassify` for both and then in `current()`; `forget` for remembered and forgotten entries,
`AgentMemory::reclassify` also for a staged reclassification. Eight `matches!` scans in all. The
fold that answers the question - `NoteChain::current`, `MemoryChain::current` - exists, but only
over the stored entries.

Beside it:

- `revoke` removes a note noted in the same turn from the staged changes instead of staging a
  revocation - the only reason `Turn::staged_channel_changes` hands out `&mut Vec`.
- `AgentMemory::forget` and `reclassify` call `recalled`, which asks the policy for every entry, to
  find out whether one entry may be recalled.
- `NoteKind::change` and `NoteKind::of` exist because `AgentChannelChange` has two variants with the
  same payload (`Note`, `PrivateNote`).

## Why it matters

Every new operation on notes or memory is another set of scans, and every new kind of change another
arm in each. The fold is the core of the ledger of [92](92-ledger-for-notes-and-memory.md); written
once, it serves both.

## Considerations

- The fold is freed from the chain in the model (`apply_to` on the change, or a free fold), so that
  a wrapper runs it over staged changes too.
- `AgentNotes::effective(turn, kind)` and `AgentMemory::effective(turn)`: the current state with the
  turn's changes folded in. `revoke`, `reclassify` and `forget` look there.
- A revocation of a note noted in the same turn is staged like any other; its number is used up
  anyway. `staged_channel_changes` becomes `&[_]`.
- `AgentMemory::find(turn, id, ...)` asks the policy for the one entry.
- Optional: `AgentChannelChange::Note { kind, change }`; better with
  [65](65-generic-component-state.md).
- After [92](92-ledger-for-notes-and-memory.md), so that the fold is written once.

## Done when

- No operation of `AgentNotes` or `AgentMemory` scans the staged changes itself.
- Forgetting or reclassifying one entry asks the policy about that entry.

## History

- **2026-09-30** — From the review of the agent actor (`work/2026-09-30-code-review-agent.md`, N1,
  N2, N4), epic [151](151-agent-refactoring-epic.md).
- **2026-10-01** — Done. The ledger's fold runs over staged changes too: `Ledger::with_staged` gives
  a `Staged<K>` (`current`, `get`, `next_id`) sharing one private fold with `Ledger::current`; an
  entry added by a staged change follows `MessageId::default()` until it is appended.
  `AgentNotes::effective(turn, kind)` and `AgentMemory::effective(turn)` are what `note`, `revoke`,
  `reclassify` and `forget` look at - no operation scans the staged changes any more. A note revoked
  in the turn that noted it is revoked by a staged `Remove`; both changes are taken over and its
  number is used up. `Turn::staged_channel_changes` returns `&[_]`. `AgentMemory::find(turn, id,
  ...)` asks the policy about the one entry (`may_recall`) - none at all for an entry remembered in
  the turn. `AgentChannelChange::Note { kind, change }` was not taken up: left to
  [65](65-generic-component-state.md), which decides how component state is held generically;
  `NoteKind::of` is used in one place now.
