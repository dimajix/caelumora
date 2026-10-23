# 405 — The context gauge keeps the old size after `/compact`

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** User interaction
**Summary:** After an agent compacted its context on `/compact`, the web UI's gauge still shows how full the context was before, until the page is loaded again.

## The problem

The gauge under each agent shows `context_tokens` of its usage (issues
[209](209-token-usage.md), [390](390-context-gauge-profile-from-history.md)). `ChatView` takes it
from the snapshot, adds each call as its message arrives (`withCall`) and fetches it again
(`readUsage`, through `api.getParticipant`) on `TurnFinished`, `TurnGivenUp` and
`ActivityFinished` - the end of an automatic compaction
([284](284-automatic-context-compaction.md)).

A compaction asked for with `/compact` is a command turn ([269](269-agent-commands.md)): it
announces no answer's start nor end, so no `TurnFinished` follows it. It ends with
`CommandExecuted`, and on that the web UI fetches nothing. The agent did store its state before
(`AgentActor::carry_out` stores, then sends `CommandExecuted`), with the estimate of the new branch
in `used.context_tokens` - a reload shows it, the open page keeps the old number.

WEB-UI.md already says the numbers are "fetched again when its turn ends or it compacted its
context"; the command path was missed.

## Related: the automatic compaction reads before it is stored

`ActivityFinished` is sent from within `handle_busy`, before the run loop stores the state after
the step (`settle`). The fetch it triggers races the store and may read the usage as it was before
the compaction. It is covered up within the turn - the next call's message replaces the number,
and `TurnFinished` fetches it again -, but it breaks the rule `end_turn` follows for `TurnFinished`
(issue 65): the state is stored before an event announces that it changed.

## Done when

- After `/compact` - and any other command an agent carries out - the gauge shows the stored usage
  without a reload.
- Whether `ActivityFinished` should follow the store, as `TurnFinished` does, is decided.

## History

- **2026-10-17** — Opened from a report of the maintainer: after a compaction, the web UI shows the
  old context size of the agent; a reload fixes it.
- **2026-10-17** — The web UI fetches the usage again on an agent's `CommandExecuted` (with a
  `turn_id`), as on `TurnFinished`; WEB-UI.md says so. Whether `ActivityFinished` should follow
  the store is still open.
- **2026-10-17** — Decided by the maintainer: it should. `handle_busy` takes the agent's state,
  goes on to `Planning` and stores it (`settle`) before it sends `ActivityFinished`, as `end_turn`
  does before `TurnFinished`. No test pins the order: in the test world the store is done before
  the event is seen either way, and a store that holds a write does not exist there.
