# 117 — `Message::prev_id` is excluded from the digest yet still set

**Status:** open
**Area:** Context & knowledge

## The problem

`Message::prev_id` (`caelumora-model/src/message.rs:25`) is deliberately
left out of the message digest "because the chain expresses it": every
chain a message is appended to sets the predecessor itself
(`Chain::push_with`, `MessageEntry`). Yet `ollama.rs:377` and
`actor_impl/agent.rs:397` set `prev_id` when they create a message, so the
field carries a value that no chain uses and no digest covers.

## Why it matters

A field that is set but never meaningful is read by the next person as
meaningful. Issue [08](08-retry-and-edit-past-answers.md) decided to fork by
`MessageId`, precisely because `prev_id` is per chain and overwritten -
that decision is easier to keep if the field does not pretend otherwise.

## Considerations

Either remove the field (the chain's `last_message_id()` answers the
question) or remove the two setters and document that the field belongs to
the chain. The first is cleaner and is what 08 implies; check what the web
UI and the context panel read.

### What `prev_id` was meant for, and why it cannot do it

The intent was that every message knows its predecessor, so that once
messages are stored individually (e.g. in Postgres,
[03](03-postgres-entity-store.md)/[04](04-postgres-state-store.md)) the
chain can be rebuilt from them. The digests alone do not allow that: an
entry stores only its own digest, which covers the digest before it, but a
hash cannot be inverted - given a set of entries one can *verify* a
candidate predecessor (`message_chain(p.digest, msg) == msg.digest`), not
*find* it. Today the order is only the position in the chain's `Vec`.

`Message::prev_id` does not close that gap, because a message is in several
chains at once: the same `MessageId` is in the channel history and in the
transcript of every agent (forwarded messages keep their id), with a
different predecessor in each - an agent's transcript has its internal tool
messages in between. That is why `push_message` (`message.rs:186`)
overwrites it per chain, and why the copies of one message carry different
values. Stored once per `MessageId`, a `prev_id` can express one of those
chains - whichever was written last. The predecessor is a property of the
*entry in a chain*, not of the message.

Besides: `prev_id` is not covered by the digest, so it is not tamper-evident
and worthless for audit; and on an empty chain `push_message` does not
overwrite it, so the first entry keeps what `ollama.rs:377` set - the last
message of the model call's context, not of the chain.

### Where the predecessor belongs

On the chain entry, as the digest of its predecessor - the way a git commit
names its parent:

```
messages      (id, role, sender, content, …)          -- once per message
chain_entries (chain_id, message_id, digest, prev_digest)
```

- `prev_digest` names the predecessor in *this* chain unambiguously.
- It is verifiable: `digest == message_chain(prev_digest, message)` - a
  guarantee `prev_id` never gave.
- It is exactly the parent pointer branching needs
  ([08](08-retry-and-edit-past-answers.md)): two entries with the same
  `prev_digest` are a fork, and the chain becomes a tree.

While chains are stored as documents, the order of the `Vec` already
expresses the predecessor, so nothing has to be added to the entries now.

## Done when

`Message` has no predecessor field that the chains do not own, and
nothing sets one. Where the predecessor will live instead (`prev_digest` on
the chain entry, added with a store that keeps entries individually or with
branching) is recorded here and referenced from 08.

## History

- **2026-09-28** — Split off from [97](97-small-model-unifications.md)
  (structure review, 3.4 F).
- **2026-09-29** — Discussed with the maintainer. The intent of `prev_id` was
  to rebuild chains from individually stored messages, which the digests
  alone cannot do; but since a message is in several chains with different
  predecessors, the field cannot serve that either. Decided: remove
  `Message::prev_id` and its setters; the predecessor belongs to the chain
  entry as `prev_digest`, to be added when entries are stored individually
  (03/04) or chains branch (08). Considerations and "done when" extended.
