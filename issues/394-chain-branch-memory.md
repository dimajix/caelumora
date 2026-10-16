# 394 — `Chain` remembers the branch last taken from every entry, for a switch nothing makes

**Status:** rejected
**Area:** Code structure

Part of [391](391-history-and-transcript-review-epic.md).

## The problem

`Chain<E>` (`caelumora-model/src/chain.rs`) became a tree with a head as step 1 of the branching
plan of [08](08-retry-and-edit-past-answers.md): beside `parent`, every `Node` carries
`active_child` - the child last on the active path, if it is not the newest - and the chain keeps
`newest_child`, so that `leaf_from(digest)` finds where a switch back to a branch goes on, as the
`< n/m >` switch of that plan needed. `children`, `seq_of` and `at` came with it. Issue 08 was
rejected on 2026-10-05 and replaced by [234](234-modify-revoke-and-retry-messages.md), which
changes a message where it stands and never moves a head back to an older entry.

What is left runs for nobody: `leaf_from`, `children`, `seq_of` and `at` have no caller outside
`chain.rs`. In production the head moves only to the root (`Transcript::start_anew`, which a
compaction and an external agent's call use), where there is no path to remember on. Yet the
mechanism is paid for everywhere a chain is handled:

- `active_child` is serialised with every node and checked in `Chain::apply` ("remembers an entry
  that does not follow it");
- `move_head` marks every entry whose memory changed as pending, so `Stored` may hold nodes that are
  not new but "changed in their place";
- both stores handle that: `kept_chain::place` puts a node into its place or after the last,
  `chains::append` reads a segment back, drops what lies past the head and places every node again,
  `KeptChain::append` likewise, and `Chain::apply` tells a node already there from a new one.

## Why it matters

- `Chain::apply`, `chains::append` and `KeptChain::append` are each about a third longer than what
  they do today needs: taking in nodes appended after the last.
- Whoever reads the chain has to understand a switch between branches to understand why a stored
  node can change - and then finds that none does.
- The Postgres store of [04](04-postgres-state-store.md) would have to store and update the field as
  well.

## Considerations

An architecture decision, since it removes a capability and changes what is stored:

- **A - Remove it.** `Node` loses `active_child`, `Chain` loses `newest_child`, `leaf_from`,
  `children`, `seq_of` and `at`; `move_head` only moves; `Stored` holds the nodes added since the
  chain was stored and the head, nothing changed in its place; `apply`, `chains::append`,
  `KeptChain::append` take in new nodes only, and `place` goes. STORAGE.md (*What a state tells a
  store*) loses "or changed in their place". Stored chains have to be deleted, as every change to
  what is stored allows (AGENTS.md). Should branching come back, the memory comes back with its
  first caller, as the rule for everything else is.
- **B - Keep it.** Nothing to do, and a plan that was rejected keeps its footprint in every store.

Recommended: A. The chain stays a tree with a head - `parent`, `move_head(None)`, `prefix` on any
branch, `all_entries` - which is what compactions, an external agent's calls and the context of an
old message need.

## Done when

- A node of a chain is stored once and never changes; `Stored` and both stores take in new nodes
  alone.
- `Chain` has no method without a caller outside its tests.
- STORAGE.md and AGENT-TURN.md (*The transcript as a chain*) describe the chain as it is.

## History

- **2026-10-16** — Opened from the review of the history and the transcript
  ([391](391-history-and-transcript-review-epic.md)), after 08 was found rejected.
- **2026-10-16** — Rejected by the maintainer, as *B - Keep it*: a viable concept for branching in a
  channel of more than two participants may yet be found, and the branch memory is what a switch
  back to a branch would go on from. Looked at once more before the decision: in production
  `active_child` is set only where an entry is appended with the digest of one already there
  (`Chain::push_with`), and read only by `leaf_from`, which, like `children`, `seq_of` and `at`, has
  no caller outside the tests of `chain.rs`; `start_anew` moves the head to the root and remembers
  nothing. What *Considerations* says of option A was put too strongly in one point: removing the
  field would have needed no stored data deleted, since `Node` ignores a field it does not know.
