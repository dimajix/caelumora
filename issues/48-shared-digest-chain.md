# 48 — History and context duplicate one structure, and notes would be a third copy

**Status:** done
**Area:** Context & knowledge

## The problem

`History` and `MessageContext` (`caelumora-model/src/message.rs`) are the same structure
twice: entries that are only appended, each with a digest over its predecessor's
([06](06-context-in-message-for-auditability.md)), `prev_id` set on append, a digest of the whole, a
lookup by digest. The append and digest code is nearly identical.

The notes of a channel ([17](17-information-sharing-within-session.md)) are the same structure
again: a chain of entries (notes and revocations) with digests. And branching
([08](08-retry-and-edit-past-answers.md)) has to turn all of them into trees with a head.

## Why it matters

Building notes as a third copy means building branching three times later, with three chances to get
the digests or the head wrong. One chain type makes the tree a single change that history, context
and notes all get at once.

## Considerations

- A generic chain over an entry type, e.g. `Chain<E>`, owning append, the digest of each entry and
  of the whole, lookup and prefix by digest. The digest scheme per entry type stays where it is
  (`caelumora_model::digest`), with its version tag; the chain takes a function or a trait for it.
- `History` and `MessageContext` keep their public interface and their serialised form as far
  as reasonable; stored data need not stay compatible (legacy data does not have to be supported).
- `prev_id` handling (each store sets it to its own last message) stays as it is, and is documented
  as per store, which matters for branching (see the plan in
  [08](08-retry-and-edit-past-answers.md)).
- Linear only. The tree (parent links, head, fork, switch) is step 13 of the order of work in
  [19](19-memory-tiers.md), not part of this issue, so that this stays a pure refactor without
  change in behaviour.

## Done when

- `History` and `MessageContext` are built on one chain type, with no change in behaviour;
  the tests from 06 and 41 pass unchanged.
- The chain type is ready to be used for a channel's notes.

## History

- **2026-09-25** — Issue created as step 2 of phase 0 of the order of work recorded in
  [19](19-memory-tiers.md).
- **2026-09-25** — Implemented in `caelumora-model/src/chain.rs`: `Chain<E: ChainEntry>` with
  `push_with` (the entry is built from the digest before it, so each entry type keeps its own digest
  scheme), `digest`, `entries`, `last` and `prefix`. It serialises as the plain list of its entries
  (`#[serde(transparent)]`), so the stored form of `MessageContext` and `History`
  (`{"messages": [...]}`) is unchanged. `MessageContext` and `History` keep their interface;
  what they share as chains of messages - the `message_chain` digest and `prev_id` - is one private
  helper over a `MessageEntry` trait. No change in behaviour: all tests pass unchanged, including
  the one that edits stored digests in the JSON.
