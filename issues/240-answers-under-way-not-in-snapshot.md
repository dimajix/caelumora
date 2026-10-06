# 240 — A page loaded during an answer does not know it is under way

**Status:** done
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
- **2026-10-06** — Decided with the maintainer and done. `MemberSnapshot::turn_under_way`, always
  given - it is read from the running channel (`ChannelLoop::turn_under_way`) without a lock held
  for long and without the store, so no `Detail` is needed; a snapshot that no events follow says
  how things stood when it was taken. `ChannelMemberResponse::turn_under_way` (absent if none).
  External agents are left out: they have no turn here to stop or ask for again, and show their
  answer once its next message comes. The race the considerations feared does not exist: the
  stream subscribes before it reads, so a turn that ends in between is not listed, and one that
  begins in between is listed and announced. The web UI seeds its answers under way from the
  snapshot instead of starting empty; the stop button, the stop and the retry icons follow. That
  the retry icon is offered for an external agent's answers is
  [242](242-retry-offered-for-external-agents.md). Tests: the member of a running channel carries
  the turn `ResponseStarted` named, and none once its turn ended; the REST response leaves it out
  when absent. REST-SERVER.md (*Snapshots*), WEB-UI.md.
