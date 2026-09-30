# 136 — A turn under way is resolved differently from a recorded message

**Status:** done
**Area:** Context & knowledge

## The problem

Since [49](49-plan-step-system-messages.md), what a model is given is
resolved by one `ContextResolver`, but through two entrances with two
kinds of input:

- `resolve(&ContextRef)` for a message taken over: all messages of its turn
  are in the transcript and have digests; a prompt part is placed after the
  digest of its entry.
- `current(&TurnState)` for the turn under way: its messages are in
  `TurnState::messages` and have no digest yet. Parts placed among them
  therefore need a second kind of reference (`TurnPromptRef`, by the number
  of messages before it), which `ContextFrame::reference` converts into
  `PromptRef`s when the turn is taken over.

For the same reason the turn keeps a `ContextFrame` next to the
`ContextRef` it records - the frame names the transcript the turn began
with, the reference the one before its message - and the knowledge read in
a turn is kept apart (`TurnState::knowledge`, `TurnKnowledge::before`) and
cut to each message at commit.

All of this exists only because the messages of a turn get their digests
when they are appended to the transcript, at commit.

## Why it matters

Two kinds of placement, two reference types and a conversion between them
are three places where the model call and what is recorded as given could
drift apart - the very thing the resolver was introduced to prevent. Every
further kind of part placed within a turn has to be taught both.

## Considerations

- **A virtual transcript (chosen with the maintainer).** The messages of a
  turn get their digest when they are pushed, computed as the chain would
  (`digest::message_chain` over the previous entry: the transcript's last,
  or the last pending one); `TurnState::messages` becomes a list of
  `TranscriptEntry`s. A view - the stored transcript followed by the
  pending entries of the turn - serves the resolver: `prefix(digest)`
  searches both. Then:
  - a part placed within the turn is an ordinary `PromptRef` after its
    entry; `TurnPromptRef` and the conversion go;
  - the turn's frame is the `ContextRef` of its next message: `push_message`
    moves `transcript` and `turn_entries` on, placing a part adds a
    `PromptRef`, reading adds to `knowledge`. `ContextFrame`,
    `TurnState::knowledge` and `TurnKnowledge` go;
  - the resolver has one method, `resolve`; `current` goes;
  - each message needs the reference that held *for it*, not the final
    one: `push_message` keeps a snapshot of the reference with the message,
    and `commit_turn` writes these to the history. (Rejected: filtering the
    final reference at commit - it would need the knowledge index back.)
  - at commit the pending entries are appended to the transcript, which
    computes the same digests again (checked); `prev_id` is not part of the
    digest ([117](117-message-prev-id.md)) and is set there as today;
  - an abandoned turn is discarded as today, with its `TurnState`; the
    stored transcript is untouched and still only grows.
- **Rejected: appending the messages to the transcript right away.** The
  same unification, but an abandoned turn would have to be cut from the
  transcript again, "the transcript only grows" would hold only for its
  committed part, and the stored transcript would hold an unfinished turn.
- **Rejected: one method `resolve(reference, pending: &[Message])`.** Moves
  the difference into a parameter; the placement by count stays.
- **Invariant: the transcript does not change during a turn.** The pending
  digests build on its last entry when the turn began. Today nothing is
  perceived during a turn - events wait in the queue - but it has to be
  stated, and `commit_turn` notices a violation through the digest check.
  Should interruptions ever come, this is where they have to be solved.
- The view is two slices, not one: `prefix` returns the stored and the
  pending part, or an iterator. The resolver splits by `turn_entries`
  anyway.
- The stored form of `TurnState` changes (`messages` with digests, the
  frame a `ContextRef`, no `knowledge`), without serde defaults
  ([108](108-serde-defaults-for-legacy.md)).

## Done when

- The turn under way and a recorded message are resolved through one
  method, from one reference type, with one kind of placement.
- `ContextFrame`, `TurnPromptRef` and `TurnKnowledge` are gone, and the
  stored transcript still only grows.

## History

- **2026-09-30** — Issue created from a question of the maintainer after
  [49](49-plan-step-system-messages.md): why `resolve` and `current`, and
  `ContextRef` and `ContextFrame`, are two. Appending the turn's messages
  to the transcript at once was weighed first; the virtual transcript was
  chosen instead, because it keeps the stored transcript growing only.
- **2026-09-30** — Done as chosen.
  - `TurnState` holds `context: ContextRef` - the reference of its next
    message - and `entries: Vec<TurnEntry>`: each message as it will be in
    the transcript (`TranscriptEntry`, with its digest) and the context it
    was produced from. `TurnState::push` computes the digest from
    `context.transcript` - always the end of the view - and moves the
    context on; `Turn::push_message` and `ToolCalls::collect` both use it.
    `TurnState::record_knowledge` adds to `context.knowledge`.
  - `TranscriptView` (`caelumora_model::context`) continues the stored
    transcript with the turn's entries; `prefix` returns the entries a
    digest names across both. `ContextResolver` holds one and has one
    method, `resolve`.
  - `commit_turn` appends the messages to the transcript and records each
    with its `TurnEntry::context`; if the transcript's digest then differs
    from the last entry's, it logs that the transcript changed during the
    turn.
  - Gone: `ContextFrame`, `ContextFrame::reference`, `TurnPromptRef`,
    `TurnKnowledge`, `TurnState::knowledge`, `ContextResolver::current`.
    `Turn::messages` returns copies.
  - **`Turn::set_context` no longer takes the transcript's digest**, only
    the parts: the turn's context begins on the transcript as it stands in
    `next_turn`, and the turn's own messages continue exactly that - a
    provider naming another transcript would be ignored from the first
    message on. Which transcript a model is given becomes a question again
    with compression ([16](16-context-compression.md)), which produces
    another context altogether. Two tests that appended to the transcript
    within a turn now perceive between turns, as the runtime does.
