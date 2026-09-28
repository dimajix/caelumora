# 72 — The recorded read of a history ignores the reader's limit

**Status:** done
**Area:** Context & knowledge

## The problem

When an agent reads a channel's history with `read_history`, the turn
records what it read as a `KnowledgeRef::History { digest }` (issue 17). The
digest is the digest of the channel's `MessageHistory` at that moment, and
it names the whole history up to that point.

Since [46](46-channel-participants.md), a participant can be added with a
limited view: none of the earlier history, the last n messages or the last n
days. `ChannelLoop::history_with_digest_visible_to` cuts the messages at
that limit, but the digest it returns is still the one of the *whole*
history. Resolved later, for example in the context panel ("Knowledge
read"), the reference shows messages the agent never saw.

## Why it matters

- What an agent saw is meant to be reconstructable exactly (issue
  [06](06-context-in-message-for-auditability.md)). Here the record claims
  more than was read.
- It is also a small leak. The owner of the agent sees in the context
  panel messages of a channel from before the agent joined, which the
  agent's limit was meant to withhold. The owner may be allowed to read that
  channel anyway, but the panel is not the place that decides it.

## Considerations

- **Record the start as well.** `KnowledgeRef::History` gets the
  `HistoryStart` (or the id of the first message read) next to the digest.
  Resolving applies it. This is the smallest change, and old references
  without it mean "all of it".
- **A digest of what was read.** Compute a digest over the visible messages
  only. This does not work with the chain: the digest names a prefix of the
  channel's chain, and a cut-off prefix is not a prefix. It would need a
  second kind of reference.
- **Resolve with the reader's current limit.** This is wrong. The limit can
  change when a participant leaves and joins again, and the record must say
  what applied *then*.

The first option fits the existing model: the digest says "up to where", the
start says "from where".

## Done when

- A recorded history read names exactly the messages the reader saw.
- The context panel shows no message from before the reader's limit.

## History

- **2026-09-28** — Issue created while moving the history limits into the
  `ChannelLoop` ([71](71-access-helpers-on-entities.md)), where it was
  noticed that `history_with_digest_visible_to` returns the digest of the
  whole history.
- **2026-09-28** — Done with the first option. `KnowledgeSource::History`
  carries `start: Option<HistoryStart>` next to the digest (absent in old
  records: all of it). `ChannelLoop` has a single public method for the
  history, `visible_history(participant) -> VisibleHistory { messages,
  digest, start }`; `history_with_digest` and
  `history_with_digest_visible_to` are gone and `history()` is
  crate-internal, as the maintainer asked for one method rather than pairs
  with and without digest. Returning the `MessageHistory` itself was
  considered and rejected: cut at a start it is no longer a valid chain.
  `read_history` records digest and start; REST passes the start on as
  `history_start`, and the context panel says "from when it was granted".
  A correction to the description: the context panel did not resolve
  history references to messages, so nothing was shown that should not
  have been - the record was wrong, not yet the display.
