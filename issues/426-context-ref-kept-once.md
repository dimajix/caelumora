# 426 — Every message a turn says is stored with its whole `ContextRef`

**Status:** in progress
**Type:** refactoring
**Priority:** medium
**Area:** Persistence
**Summary:** Every message a turn says is stored in its channel's history with the whole `ContextRef` it was produced from - a digest per prompt part, the knowledge read, the model profile - repeated nearly unchanged from message to message, often larger than the message itself.

## The problem

Since [417](417-user-history-from-its-actor.md) a message an agent sends carries what it was
produced from (`Message::produced_from`), and its channel's history records it with the message
(`HistoryEntry::Said`). The texts of the prompt parts and the tool sets it names are kept once by
digest ([383](383-prompt-texts-and-tool-sets-kept-per-turn.md), `Kept`, `<agent>.given/`), but the
`ContextRef` itself is serialized in full into every message:

- the digest of the transcript before the message,
- for every prompt part a `PromptRef`: the digest of its text, its role, the digest of the entry it
  follows and its classification - two 64-digit hex strings per part,
- the digest of the tool sets offered,
- every `KnowledgeRef` read in the turn so far - channel, session, source with its digest,
  classification,
- the `ModelProfile`, four enum values.

With five to ten prompt parts that is roughly 1-2 KB per message (estimated, not measured), more
with knowledge read - often more than the message's content. Most of it repeats: the profile
nearly never changes, the tools and most prompt parts stay the same within a turn, and the
knowledge read grows with the turn and is written again, whole, with each of its later messages.

## Why it matters

A history grows without a bound (STORAGE.md, "Histories"); what each message adds to it is paid in
every store, every load of a window and every scan for a message (`JsonFileStore` scans items
backwards). Of everything recorded with a message, its context is the part read the least: only
when someone asks what the model saw (`view/context.rs`) or a turn resumes
(`turn/mod.rs`).

## Considerations

- **A: the `ContextRef` kept once by digest, beside the prompt texts and tool sets.** The message
  stores only its digest (`produced_from: Option<Digest>`); the reference is kept in `Kept` as the
  texts are (`context-<digest>.json` in `<agent>.given/`, a table keyed by agent, channel and
  digest in Postgres), written first with them when the agent's state is put, read through the
  `TranscriptStore` before `named_by`. 64 bytes per message; repetition within a turn and across
  turns falls away by itself. Costs: a digest of a `ContextRef` (`crate::digest`), one more read
  for whoever resolves an older context, `keep_window` holding the reference of the turn under
  way, and the readers of `produced_from` that take fields of it directly (`turn/context.rs`,
  `turn/mod.rs`) going through the store or the turn's work.
- **B: a reference relative to the previous message of the same turn.** Only what changed is
  stored. Smaller still, but a context could no longer be read without the messages before it in
  the turn, and a message taken back or a turn cut off breaks the chain.
- **C: only the profile kept apart, or left out when it is the default.** Cheap, but takes away
  only a small part of the repetition.

A is the pattern the store already has for what a context names (383); a reference to such things
being kept the same way is consistent. The history's digest does not cover `produced_from`, so no
digest scheme of the history changes. `produced_from` is never published and the REST API does not
show it, so the web UI is not affected.

Stored histories with the whole reference in their messages have to be deleted (AGENTS.md: no
compatibility code).

## Done when

- A message in a history names what it was produced from by a digest alone, and the
  `ContextRef` is kept once per agent, channel and digest.
- What the model saw for an older message, and a turn resumed after a crash, resolve as before;
  the tests of `view/context.rs` and of the turn show it.
- STORAGE.md ("Prompt texts and tool sets") and AGENT-TURN.md say what is kept where.

## History

2026-10-18: Opened from a question of the maintainer how `produced_from` is stored. Estimate of
1-2 KB per message from the shape of `ContextRef`; no stored data measured.

2026-10-18: Decided: A′ instead of A. A as written saves nothing in all: `ContextRef::transcript`
moves on with every message said (`AgentChannelState::said`), so no two messages have the same
reference, and keeping it whole by digest only moves it out of the history - one kept value, and
in the `JsonFileStore` one file, per message. What repeats is everything besides the transcript,
which the turn sets: its prompt parts (`set_context`, a step of a plan), its tools
(`offer_tools`), the knowledge it read, its profile and time zone. So the `ContextRef` is split
into the transcript and a `FrameRef` holding the rest, and only the `FrameRef` is kept once by
digest; a message names the digest of its transcript and that of its frame. Rejected besides:
keeping `ContextRef` whole and leaving out its transcript only where it is kept and digested - a
reference whose `transcript: None` already means "empty" would then resolve, wrongly and without an
error, wherever the transcript was not put back. The message stays flat when serialized; both
digests are fields of their own rather than a struct under `#[serde(flatten)]`, which would read
every message through serde's buffer. `keep_window` need not hold the frame of the turn under way:
the turn holds its context whole in `Work::context`. The split is done first, without a change of
behaviour.
