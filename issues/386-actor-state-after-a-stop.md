# 386 — Whoever follows a channel keeps an agent's last state after its session stops

**Status:** done
**Area:** Sessions and channels
**Summary:** Whoever follows a channel keeps an agent's last state and its answer under way after the session stops, and the follower's reconnect starts the session again.

## The problem

Since [377](377-stopping-a-session-aborts-its-actors.md), a stopped agent keeps its turn: it stores
the step it stands at - `Planning`, `ExecutingTools`, `WaitingForUser` - to be resumed, and
announces nothing as it ends. The views already report no state for a member of a channel that does
not run (`MemberView::state`, issue 243): a snapshot taken after the stop is right.

Whoever follows the channel live is not. The web UI keeps a member's state from the last
`StateChanged` it saw (`ChatView`), and the answer under way from its `ResponseStarted`; neither is
ended, since the stopped agent sends neither a state nor `TurnFinished` or `TurnGivenUp`. Its user's
actor ends with the channel, and with it the stream (`join_channel`, issue 104) - without saying
why. Until the client reads the channel again, the agent is shown thinking in a session that does
not run.

What happens then is a second question. `EventSource` reconnects by itself after a stream ends,
and `join_channel` reaches the channel through `ensure_session`, which opens the session - loads it
from the store and starts it. So a session stopped by hand is started again by every browser that
still follows one of its channels, and the fresh snapshot is what clears the stale state.

## Why it matters

A stop that a follower undoes at once is no stop, and an orderly shutdown
([297](297-orderly-shutdown.md)) would have every open browser race the shutdown. And a state shown
for an actor that does not run tells the user something false: that an answer is on its way.

## Considerations

- **Not a state `Stopped`.** Discussed in 377 and left out: `ActorState` is also the stored
  `processing_state` a resume starts from, so a stopped agent has to keep `Planning` or
  `ExecutingTools` stored. Whether an actor runs is its channel's to say (`channel::STOPPED`).
- **The stream says why it ends.** As it says `expired` for a token about to expire (`sse.rs`), it
  could end with an event saying the channel stopped - or was deleted, or the user left it. The
  client then clears every member's state and the answers under way, and does not reconnect by
  itself; it reconnects when the user acts. Needs the runtime to tell the user's actor why it
  ends, which `stop` and `leave` now distinguish.
- **Or the client listens to `channel::STOPPED`** on the global stream and clears the channel's
  states itself. Keeps the channel stream as it is, but two streams then have to agree.
- **Whether following a channel may start its session** at all - or only a request that acts in it,
  such as sending a message - is to be decided with this; it decides whether a stop holds while
  someone watches.

## Done when

- After a session stops, nobody who followed one of its channels is shown a state or an answer
  under way of an actor in it.
- A session stopped by hand stays stopped while browsers follow its channels, unless someone acts
  in it - or it is decided and written down that following starts it.

## History

- **2026-10-16** — Opened after [377](377-stopping-a-session-aborts-its-actors.md), which left the
  question of a state `Stopped` open. That a follower's reconnect starts the session again was
  found in the code while looking at it (`ensure_session`), not yet tried in a browser.
- **2026-10-16** — Decided with the maintainer and done. The REST server's channel stream follows
  the user's actor across its runs and does not start the channel: nothing new is said in a channel
  that does not run, and whoever acts in it starts it.
  - What the user's actor shows goes to an outbox that outlives it, kept by a new `ActorManager`
    per channel and actor; the actor of the next run shows to it again. A frontend reaches it
    through the views (`ChannelView::user_follow`), as a participant, running or not.
  - The actor announces each run in it, `Shown::Started` and `Shown::Ended`, after everything it
    showed; one ended before it ran shows its end alone. So the stream needs neither the event bus
    nor the actor's end: at either marker it reads the channel through the views again and sends
    the snapshot - at a stop without a member's state or an answer under way -, or `started` and
    `stopped` to a client without snapshots, and ends once the user takes no part any more.
  - The actor now shows what its user says as it forwards it, no longer as it queues it, so that it
    falls within its run: the message that starts a channel arrives after the snapshot of the start.
    Found while doing this: with the stream attaching to the new actor from outside, that message
    got lost to the writer's own stream.
  - Not covered: a session deleted while it does not run has no actor to say so; a stream following
    it ends with its token, the reconnect is answered `404`.
  - Found on the way: the REST server has no endpoint that stops a session by hand, so the case
    arises with an orderly shutdown ([297](297-orderly-shutdown.md)) and with unloading idle
    channels ([381](381-inactive-channels-held-in-memory.md)); 381 no longer counts a stream
    subscribing among what starts a channel. `list_commands` and `candidates` still start the
    session, since they need its running agents.
  - Changed behaviour: an open browser no longer resumes a session after a restart of the server;
    whether the server should is noted in 297.
