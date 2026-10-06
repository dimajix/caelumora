# 235 — An answer still being generated cannot be asked for again

**Status:** open
**Area:** Sessions & channels

## The problem

A user can ask an agent to answer again only once its answer is complete
([234](234-modify-revoke-and-retry-messages.md)): the web UI offers the retry icon on a stored
message while nothing is being generated, and an agent reads its inbox only between turns, so a
request sent during a turn waits for the turn to end. An answer that is visibly going wrong - a
derailed plan, a tool loop - has to be stopped first with the channel's stop button, which ends the
turns of *every* agent in the channel, and the stopped answer is then asked for again.

## Why it matters

The answer that goes wrong while one watches is the one most often wanted again. Stopping the whole
channel to retry one agent's answer is a detour, and in a channel with several agents answering at
once it stops answers nobody wanted stopped.

## Considerations

- The maintainer's wish when 234 was planned: retry only a completed single message, or - if it is
  not completed - cancel the current turn first.
- `ChannelLoop::cancel` acts on the channel as a whole (`CancelChannel`); cancelling one actor
  (`Actor::cancel`) is crate-internal and checks nothing, as it is the consequence of an act that
  was checked (SECURITY.md). A retry of an answer under way needs an operation that cancels the
  author's turn as part of asking it to answer again, asked as `RetryMessage` - not a way around
  the channel's cancel.
- The message being streamed has an id (`PartialMessageGenerated` carries one); whether it is the
  id the complete message will have, and so one a request can name, has to be checked. A cancelled
  turn is taken over, so its partial answer becomes a message of the history - which the existing
  retry then replaces.
- What the user sees in between: the cancelled answer appears, then is taken back. Acceptable, or
  should the UI hide it?

## Done when

A user can ask for another answer while the agent is still answering; only that agent's turn is
ended, and its answer is replaced as a completed one is.

## History

- **2026-10-06** — Opened from open question 5 of [234](234-modify-revoke-and-retry-messages.md),
  which built retry for completed answers only.
