# 391 — Epic: review of history and transcript

**Status:** open
**Area:** Maintenance

## The problem

After the work of [376](376-histories-held-whole-in-memory.md) to
[390](390-context-gauge-profile-from-history.md) the history and the transcript - the `Chain`, its
two entry types, the `HistoryStore` and `TranscriptStore` with both backends, the
`ContextResolver`, `AgentChannelState` and their readers in the agents, the runtime and REST - were
reviewed on 2026-10-16 for what can be simplified, what is written twice and what has no caller
left. The structure is sound and the documents match the code. What is left over are readers from
earlier states that nothing in production calls any more, a mechanism built for a plan since
rejected, two places where the same rule stands twice, and a few words that mean several things.
This issue is the bracket around the issues that clean that up, and their order.

## Why it matters

- Every reader of a history or transcript that stays, though nothing calls it, is a second way of
  reading the same thing - and the next change has to keep both right.
- `HistoryEntry` and `TranscriptEntry` apply a change of a message by the same rule, written twice;
  the digest of such a change is mapped twice as well. A change to the rule - a field a
  modification takes over, say - has to be made in four places.
- The branch memory of `Chain` (`active_child`, `leaf_from`) was built for the branching of
  [08](08-retry-and-edit-past-answers.md), which was rejected and replaced by
  [234](234-modify-revoke-and-retry-messages.md). It costs a serialised field, a pending mark on
  every head move, a check in `Chain::apply`, and "a node changed in its place" in both stores -
  for nothing that runs.

## Recommended order

1. [392](392-history-readers-without-callers.md) - the readers nothing calls and the stale
   comments: deletions without a change of behaviour, which shrink the surface for what follows.
2. [393](393-history-record-wrappers.md) - one `record` on each chain instead of five wrappers that
   only assert the variant.
3. [394](394-chain-branch-memory.md) - the branch memory of `Chain` and "changed in its place" in
   the stores, after the maintainer decided it goes. Early, since it shortens `Chain::apply` and the
   stores before anything else touches them, and stored chains have to be deleted once.
4. [395](395-message-change-applied-twice.md) - one `MessageChange` applied and digested by one
   rule, for both entry types.
5. [396](396-three-ways-to-read-a-message-as-it-stands.md) - `History::message` and
   `Transcript::standing` read as `known` does; `fold` stays for ranges alone. After 395, on the one
   rule it introduces.
6. [397](397-agent-channel-state-follows-the-transcript.md) - the turn's context follows the
   transcript's head in one place; `compact` without the four checks for a turn;
   `Turn::compact` given what it recomputes.
7. [398](398-given-means-four-things.md) - the names: `TranscriptStore::given`, the `.given`
   directory. Last, since it only renames, and after everything that would have to be renamed with
   it.

## Considered and not taken up

- **One `fold` for both chains.** `history::fold` and `transcript::as_they_stand` both read a range
  into the messages as they stand, but by different rules - the history on whichever branch and
  with the channel's fitting, the transcript along a path with what was taken back left out. The
  difference is deliberate (AGENT-TURN.md, *Changes to a message*); what they share is the
  application of one change, which 395 moves to one place.
- **`HistoryEntry::of_event`.** `ChannelLoop::append_history` maps `ActorEvent`s to the entries of
  the channel's history, and `AgentChannelState::finish_turn` and `give_up_turn` build the same
  ends for an agent's. A constructor in the model, where both types lie, would move the mapping to
  the owner of both. Small gain; noted on 393, to be weighed there.
- **`History::turn_began`.** It finds where a turn's entries begin by the end of the actor's turn
  before, scanning back from the end. Tracking the first message of the turn while scanning would
  do the same with less to explain, but the gain is small and the semantics subtle. Left as it is.

## Done when

Every issue of the order above is `done` or `rejected`, and this one records what changed in the
order along the way.

## History

- **2026-10-16** — Created at the maintainer's request from the review of the history and the
  transcript, with 392 to 398 opened from its findings.
- **2026-10-16** — 392 done: the readers of a history held in memory that only tests called live in
  `history::testing`, behind a new `test-support` feature of `caelumora-model`. Next in the order:
  393.
- **2026-10-16** — 393 done: one `record` on each chain; `HistoryEntry::of_event`, mentioned above,
  was weighed there and not taken up. Next in the order: 394, which waits for the maintainer's
  decision.
- **2026-10-16** — 395 rejected: the rule a change of a message is applied by is written once
  already, in `Message`; what stands twice is a dispatch of three lines, and either form of a
  `MessageChange` would add more than it takes away. 396 goes ahead on the two dispatches as they
  are, as it says it can.
- **2026-10-16** — 396 done: one message as it stands is read as `known` reads it, in the history
  and the transcript alike. Next in the order: 397; 394 still waits for the maintainer's decision.
