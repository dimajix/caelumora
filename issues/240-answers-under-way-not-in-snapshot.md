# 240 — A page loaded during an answer does not know it is under way

**Status:** open
**Area:** User interaction

## The problem

A client learns that an agent answers from `ResponseStarted`, which names the turn
([235](235-retry-of-an-answer-under-way.md)). The channel's snapshot - what a page is given when it
loads or falls behind - says nothing of the answers under way. A page loaded during an answer
learns of it only with its next message, partial or complete, which carries the turn's id
(`Message::turn_id`). An agent that does not stream - or one that thinks for a long time before its
first chunk - shows nothing until it is done: no "generating", no stop icon of its own, and no way
to ask for another answer while it is under way.

## Why it matters

The answers worth stopping or asking for again are the long ones, and a reload, a second tab or a
user who joins in the middle are exactly the moments a page knows nothing of them. The channel's
stop button, shown only while something is generating, is missing too.

## Considerations

- `ChannelLoop::turn_under_way(actor)` answers already for every actor of the channel; the
  snapshot could list, per member, the turn under way (`ChannelMemberResponse`), read as the
  snapshot is assembled. What happens between the snapshot and the first event is the known race of
  snapshots (REST-SERVER.md, *Streams*): an answer that ends in between is listed as under way until
  the next event of its agent.
- What streamed before the page was loaded is not kept anywhere; the page would show the answer as
  under way, empty, until the next chunk. Keeping it is a bigger matter and not needed for stopping
  or asking again.
- An external agent has no turn here (`Actor::turn_under_way` is `None`), though it answers; whether
  its answer under way is listed too - without a turn, for "generating" and the stop icon only - is
  to be decided.

## Done when

A page loaded while agents answer shows their answers as under way, with the stop and retry icons,
before their first new message.

## History

- **2026-10-06** — Opened from open question 2 of [235](235-retry-of-an-answer-under-way.md).
