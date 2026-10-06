# 237 — A message written by mistake cannot be erased

**Status:** open
**Area:** Context and knowledge

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

## Done when

It is decided whether and how a message can be erased, by whom, and what of what was derived from
it goes with it; and the documentation says what taking back and erasing each do.

## History

- **2026-10-06** — Opened from open question 9 of [234](234-modify-revoke-and-retry-messages.md),
  whose taking back is recorded as "not erasing" in CHANNELS.md and CONFIDENTIALITY.md.
