# 237 — A message written by mistake cannot be erased

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Context and knowledge
**Summary:** A message written by mistake can be taken back but not erased: it stays in every participant's record and in the contexts built on it.

## The problem

Since [234](234-modify-revoke-and-retry-messages.md) a user can take a message back. Taking back is
deliberately not erasing: the message stays in the chains of every participant that heard it, in
the contexts of the answers produced from it, in what agents noted or remembered of it, and with a
remote agent that was given it. A secret pasted into a channel by mistake - a password, personal
data - therefore stays readable to whoever may read the agents' contexts, and stays in storage.

## Why it matters

Pasting the wrong thing is common, and "taken back" can be mistaken for "gone". Some content has to
be removable for good, for security and for data protection.

## Considerations

- Every chain is append-only and every digest is stored, never recomputed ([48](48-shared-digest-chain.md),
  [197](197-one-chain-one-model-of-change.md)). Erasing content breaks that by design: a context
  that included the message can no longer be resolved as it was (`ContextError::Unavailable`), and
  that has to be acceptable for erased content.
- Who may erase: the author, the channel's owner, an administrator ([125](125-invisible-admin.md))?
  It reaches into other actors' state, so it is not a request to an agent like retry.
- What is erased: the content in the history and transcripts, but also what was derived from it -
  notes, memory, knowledge read by other channels - which cannot be found by message id alone.
- The recorder ([05](05-postgres-event-recorder.md)) will hold the events; an erasure has to reach
  it too, or be impossible there.

## Proposal

Analysed on 2026-10-06; not decided yet, the open questions are at the end.

### Where a message's content is

| where | found by its id |
|---|---|
| the channel's history (`ChannelState`) | yes |
| every agent's history, transcript and turn under way (`AgentChannelState`), former members' included | yes |
| the original of a modified message, in its `Said` entry beside every `Modified` | yes |
| the `JsonFileStore`'s journal lines before the last snapshot | only by writing the state anew |
| contexts (`ContextRef`) | hold digests only, resolved through the transcript |
| what was derived: answers quoting it, tool arguments, notes, memory, prompt part texts, readings of other channels | no |
| a remote agent that was given it ([236](236-external-agent-not-told-of-changes.md)) | out of reach |
| the recorder ([05](05-postgres-event-recorder.md)) | records nothing yet (`BlackholeRecorder`) |
| logs (content only at `trace!`), clients that show it | - |

### How content is erased

- **A. In place (proposed).** Every entry about the message - its `Said` or `Message`, every
  `Modified` and `Reclassified` - loses what it says; its place, id, sender and time stay, with
  when it was erased. Digest and parent stay as stored: that they no longer fit the content is
  harmless, since nothing recomputes a digest, and every later digest and every `ContextRef` stays
  valid. A context then resolves with the message as erased, not as `ContextError::Unavailable`.
  Costs: the one deliberate exception to "nothing changes an entry after it was added", kept to a
  single method (`Chain::erase`); `Chain::apply` has to take over a node whose entry changed, not
  only its `active_child`; and a store that keeps a journal has to write the state anew and empty
  the journal, or the content stays in the lines before - so a state has to be able to tell its
  store to write it whole.
- **B. Crypto-shredding.** Every content encrypted with a key of its own, erasing deletes the key.
  The usual answer for stores that are only appended to, but a key store and a rebuild of every
  store for this alone. Not now; it is the requirement to put to the recorder
  ([05](05-postgres-event-recorder.md)), which has to support erasing in some way.

### How it runs

As taking back does: the user asks through their actor, which sends `ActorEvent::MessageErased`;
the channel admits it in the sender's name and erases it from its history; every agent erases it
from its own chains, between turns if a turn under way holds it; the clients replace the message
by a placeholder. A message taken back or modified can be erased as well - the likely case: taken
back first, then found not to be enough.

**Former members** have no actor running, but their state holds the content. Proposed: the
`ChannelLoop` loads, erases and stores their states itself. It is the only one that starts actors,
so a former member added again cannot write meanwhile - but it extends "one writer per state"
([STORAGE.md](../docs/STORAGE.md#one-writer-per-state)) by that case, which is an architecture
decision.

### What it is not

What was derived from the message stays - notes, memory, the agents' answers - since it cannot be
found by the message's id; so do a remote agent's copy, backups and logs. The documentation says
so, and the UI's confirmation names it. An answer that quotes the secret can be erased on its own
if the channel's owner may erase others' messages (an agent's as well). Erasing notes and memory is
a follow-up issue.

### Open questions

1. Mechanism A, with the store writing the state anew, and erasing recorded as a requirement of
   [05](05-postgres-event-recorder.md)?
2. Who may erase: the author and the channel's owner, as a new `ActionType::EraseMessage`? An
   administrator would follow through the policy with [125](125-invisible-admin.md).
3. May the `ChannelLoop` write the states of former members, as a documented extension of "one
   writer per state"?
4. The scope: the message with every version of it, what was derived documented and left to a
   follow-up issue?

## Done when

It is decided whether and how a message can be erased, by whom, and what of what was derived from
it goes with it; and the documentation says what taking back and erasing each do.

## History

- **2026-10-06** — Opened from open question 9 of [234](234-modify-revoke-and-retry-messages.md),
  whose taking back is recorded as "not erasing" in CHANNELS.md and CONFIDENTIALITY.md.
- **2026-10-06** — Analysed with the maintainer: where a message's content is kept, erasing in
  place against crypto-shredding, how it would run, and the open questions (see *Proposal*). Not
  to be implemented yet.
