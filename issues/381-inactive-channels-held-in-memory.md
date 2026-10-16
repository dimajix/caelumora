# 381 — A channel nobody speaks in stays in memory with its actors

**Status:** open
**Area:** Sessions and channels

## The problem

Once a session is started, every one of its channels runs its `ChannelLoop` with its state, its
actors and their states, until the session is stopped or deleted. A channel in which nobody has
spoken for days keeps all of it: the loop's tasks, the agents' states with their histories and
transcripts, the external agents' remote state, the user actors. Nothing notices that a channel is
idle, and nothing gives it back.

## Why it matters

- A server meant to run for months accumulates every session anybody started and did not stop; the
  memory it holds is the sum of all of them, not of the ones in use.
- A turn is the one thing that may take minutes; between turns a channel's actors do nothing but
  wait, which costs memory and tasks for no work.
- It becomes worth doing once loading a channel again is cheap - which is what
  [379](379-history-stored-as-it-stands.md) and [380](380-transcript-window.md) make it.

## Considerations

- **What idle means.** No message, request, approval or command in the channel for a configured
  time, and no turn under way in it: an agent waiting for an approval is not idle, and a turn is
  never cut off for this (see [377](377-stopping-a-session-aborts-its-actors.md)).
- **What unloading is.** The channel is stopped the way a session's stop stops it - actors ended
  between turns, state stored - and the session keeps the channel's entry without its loop. A
  session whose channels are all unloaded may be unloaded itself: its controllers, its groups' live
  copies. The entity is untouched; a stopped session and an unloaded one differ in that the latter
  starts again on its own.
- **What starts it again.** A request to the channel over REST, an event addressed to it, an
  external agent's answer arriving. Not a user's stream: it follows the user's outbox, which
  outlives the actor, without starting the channel, and the actor of the next run shows to it again
  ([386](386-actor-state-after-a-stop.md)) - so unloading is unnoticed by whoever follows. The
  place is the one that today answers "the session is not running" with an error; it starts the
  session instead, under the same per-session lock that starting and stopping take (STORAGE.md,
  *one writer per state*).
- **The rule "a session runs once"** stays: an unloaded session is one that is not running, and
  the lock serialises the start. What must not happen is a start from two places at once, or an
  unload while a start is under way.
- **Events a sleeping channel would have heard** - an agent deleted or restored, a channel
  reclassified - are what a started session takes in anyway, since it reads its entities when it
  starts; what it would have *done* on them (cancelling actors on a lowering) is to be checked.
- **Agents in several sessions** are not unloaded with one session; that is
  [382](382-agents-held-without-a-session.md).

## Done when

- A channel idle for the configured time is stopped with its actors and its state stored, and starts
  again when something addresses it, without anyone noticing but in the server's memory and logs.
- A session whose channels are all unloaded is unloaded.
- A test shows a message sent to an unloaded channel arrives, is heard by its agents, and is
  answered.
- CHANNELS.md and CONFIGURATION.md say when a channel is unloaded and what wakes it.

## History

- **2026-10-16** — Opened as part of the epic [378](378-resource-management-epic.md).
- **2026-10-16** — With [386](386-actor-state-after-a-stop.md), a user's stream no longer starts a
  channel; it is taken out of what starts one again.
