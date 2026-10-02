# 193 — An external agent's state is rewritten whole for everything it hears

**Status:** open
**Area:** Persistence

## The problem

`ExternalAgentActor::hear` stores the whole `ExternalAgentChannelState` for every
`FullResponseGenerated` in its channel. Since commit `7e22f5b` (issue
[179](179-external-agents-via-a2a.md)) that state keeps everything heard - tool results and a plan's
inner steps too - and a transcript holding a second copy of every message given, where a single id
was kept before. This was deliberate ("for the record").

## Why it matters

Each write is as large as the state, so the writes over a channel's life grow with the square of
its length; tool results can be large.

## Considerations

- Keep only what may be given or is needed for the record.
- A transcript of ids into the history instead of copies.
- An append operation on the `StateStore` - needed by any long-lived state, and natural for the
  Postgres store of issue [04](04-postgres-state-store.md).

## Done when

- Hearing a message costs a write independent of the length of the channel's history.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series; a trade-off against the
  decision of `7e22f5b`, to be weighed by the maintainer.
