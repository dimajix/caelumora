# 220 — A user's stream is the channel's broadcast, not what their actor receives

**Status:** done
**Area:** User interaction

## The problem

Since [104](104-sse-stream-and-subscribe.md), a user follows a channel through their own actor
(`UserActor::watch()`), but what the stream carries is the channel's broadcast
(`ChannelLoop::subscribe()`), not what the moderator routes to the user. The user's actor passes
what it receives to a single subscriber, which only the TUI uses; for the web UI it is dropped.

So what a user sees is not decided by the moderator:

- The moderator can send nothing of its own to the users. The `ChainCutOff` of
  [200](200-chain-limit-hint.md) is forwarded to the users' actors and never reaches the web UI,
  since only the original event is broadcast.
- `RouteDecision::broadcast` exists only to keep things out of that broadcast: introduced on
  2026-07-17 (`ab7634f`) so that the internal hop of an approval - agent to approver, approver to
  agent - does not show in the UI.
- Streaming and progress (`PartialMessageGenerated`, `CompleteMessageGenerated`, `Intermediate…`,
  `ResponseStarted`, `PlanStep*`) are routed to nobody and reach the web UI only by the broadcast;
  the TUI, which reads the actor, sees none of them. The two frontends see different things.
- A user's own message reaches the web UI only by the broadcast too: the channel sends nothing back
  to its sender.

The channel's broadcast has no other user in production: besides `UserActor::watch()` for
`join_channel` only tests read it (`channel.rs`, `session.rs`, `security_suite.rs`). The global
stream runs over the `EventBus`, the recorder records each delivery itself.

## Why it matters

- What a user sees should be what is routed to them: one owner, the moderator, and no second path
  with a filter of its own.
- New events of the moderator (200) or for users only would otherwise need another way into the
  broadcast.

## Considerations

- **Several watchers:** one `UserActor` serves a user in a channel, but several streams may follow
  it - two tabs, two devices, the overlap of a reconnect. With the actor's single subscriber the
  newest would silently take the stream from the others. So the actor gets a small broadcast of
  its own; `watch()` hands out a receiver of it, and the lag handling of the SSE stream
  (`resync_on_lag`) keeps working, since it reads a `broadcast::Receiver` already.
- **A user's own messages:** as an agent takes its own messages into its history and transcript
  itself, a user's actor publishes them to its own broadcast itself; the channel keeps not sending
  anything back to its sender, and neither moderator nor channel needs an exception. The channel
  resolves `@`-mentions as a message enters it; the actor resolves them with the channel's index
  before it publishes, so that its own mentions are highlighted too. The channel can still drop a
  message the actor published (`admits`), but `send_message` asks the same permission first, so
  only a reclassification in between does - logged, and gone on reload.
- **What the moderator routes to users:** streaming and progress as well, besides what they get
  today. Agents get none of them.
- **Without the channel's broadcast:** `RouteDecision::broadcast`, `ChannelLoop::event_tx` and
  `subscribe()` go. Tests about what users see listen at a user's actor; for channel-internal
  traffic a tap stays under `#[cfg(test)]` only.
- **Inbox:** streaming then passes the actor's inbox (100 events, dropped when full); the actor
  passes them on at once, and a stream that falls behind on its broadcast is resynced as today.

## Plan

1. The user's actor gets a broadcast of its own in place of its single subscriber, which the TUI
   reads. `watch()` keeps reading the channel's broadcast until step 4: before step 3 the actor
   receives no streaming. No change for the web UI.
2. The user's actor publishes its own messages itself, with mentions resolved.
3. The moderator routes streaming and progress to the users.
4. `watch()` and with it the REST stream follow the actor's broadcast; `RouteDecision::broadcast`
   and the channel's broadcast go.
5. A test that `watch()` delivers a `ChainCutOff` (200), the docs, and a check in the browser.

## Done when

- A user's stream - web UI and TUI alike - is what their actor receives and publishes, from any
  number of watchers.
- The channel has no broadcast in production, and `RouteDecision` no `broadcast`.
- A cut-off chain shows in the web UI.
- CHANNELS.md and REST-SERVER.md say so.

## History

- **2026-10-04** — Issue created with the maintainer, after the `ChainCutOff` of 200 did not reach
  the web UI. Decided: the stream follows the user's actor, which gets a small broadcast of its own
  for several watchers and publishes its own messages itself, like an agent; the channel keeps not
  sending anything back to its sender.
- **2026-10-04** — Step 1: the user's actor passes what it receives to a broadcast of its own
  (`SUBSCRIBER_CAPACITY` 128, a power of two as for the `EventBus`), from which `subscribe()` hands
  out any number of receivers; one that falls behind loses the oldest and learns how many. The TUI
  reads it and says when it missed events. `watch()` and with it the web UI still read the
  channel's broadcast.
- **2026-10-04** — Step 2: the user's actor shows everything it sends into the channel to those
  who follow it, but the request to wake the channel (`UserActor::send`): its messages - with
  their mentions resolved by the channel's index (`ChannelLoop::mentions_in`), which the channel
  uses as they enter it, too - its answers to approvals, and a command's answer for the whole
  channel, which its issuer otherwise would not receive once the stream follows the actor, since
  the REST reply leaves it out. A user's actor sends no partial messages; were it to, they would be
  shown the same way. The TUI leaves out what its own user sent, which it shows as typed.
- **2026-10-04** — Step 3: the `policy_moderator` routes how an answer comes about -
  `ResponseStarted`, partial and complete messages, intermediate ones, plan steps - to every user
  of the channel, and to no agent: the agents hear the answer. The channel still sends nothing to
  the sender, so a user's own complete message comes only from their actor (step 2). The TUI,
  which now receives complete messages too, prints each message as it is complete, as the web UI
  shows it, and no longer the full response, which is what the agents hear.
- **2026-10-04** — Steps 4 and 5, done. `UserActor::watch()` hands out a receiver of the actor's
  broadcast (no longer fallible: it does not need the channel), and the REST stream and the TUI
  follow it. `RouteDecision` is gone with its `broadcast`: `Moderator::route` returns the
  forwards, and the channel delivers exactly those. The channel keeps a tap of everything it takes
  in only under `#[cfg(test)]` (`ChannelLoop::observe`), for its own tests of traffic no user sees;
  the session tests and the security suite follow a user's actor. The moderator's tests of
  approvals and notices now check to whom an event goes instead of the flag. A test follows a
  user's actor through a channel run by the `policy_moderator` and sees the `ChainCutOff` (200).
  CHANNELS.md, REST-SERVER.md, TOOLS-AND-COMMANDS.md and CODING-GUIDELINES.md say so. Checked in
  headless Chromium against a copy of the local data, with `max_agent_hops: 0`: in two tabs of one
  user, the own message with its mention highlighted, the answer streaming, the cut-off chain and a
  `/wakeup` run in the other tab all show; no console errors.
- **2026-10-04** — A regression found later: with the stream following the user's actor, a
  `FullResponseGenerated` with nothing to hear for the agents - a turn cancelled before it said
  anything, one of tool calls only - no longer reached the users, since the moderator forwarded it
  to nobody; the web UI kept showing the answer as under way
  ([225](225-cancel-leaves-answer-generating.md)). Fixed with step 3 of
  [224](224-what-agents-hear-of-others.md): the users always get it.
