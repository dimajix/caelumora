# 34 — An event stream starts in the middle: no snapshot, and a gap before it

**Status:** open
**Area:** Sessions & channels

## The problem

Both event streams deliver only what happens from the moment they connect,
and neither tells the client what the state was before that.

The web UI works around it in the channel stream by loading the history
first: `ChatView` calls `GET …/messages`, then opens the stream. Everything
that happens between those two calls is lost — and the window is not small:
the request has to travel, the store has to answer, the stream has to be
established. A reply that arrives in that moment is simply missing from the
conversation until the page is loaded again.

The global stream has the opposite problem: it *only* has events, and its
events carry ids and nothing else (`SessionCreated(SessionId)`), so a client
must fetch the details for every one of them. There is no way to ask "what
is there right now".

Two more cases produce the same silent gap:

- A slow client overflows the channel's broadcast buffer (100 events). The
  server currently answers that with an SSE comment saying events were lost;
  the client cannot tell what it missed.
- A stream that ends and is reconnected by `EventSource`, which is what
  [33](33-sse-outlives-its-token.md) would make a regular occurrence.

## Why it matters

It is the difference between a UI that is occasionally, silently wrong and
one that is not. A missing message in a conversation is not a cosmetic
defect: the user answers something they did not see, or waits for an answer
that already arrived.

It also decides how [33](33-sse-outlives-its-token.md) can be done at all.
Ending a stream when its token expires is only acceptable if reconnecting
restores the full state.

## Considerations

### Subscribe first, then read the state

The order is what makes it correct: subscribe to the broadcast (from then on
nothing is dropped), *then* read the snapshot, then send the snapshot
followed by the buffered events. The reverse order re-creates the gap it is
meant to close.

This means an event can arrive twice — once inside the snapshot, once as an
event — for the short moment between subscribing and reading. Duplicates are
harmless and the client can drop them by id (`MessageId` in a channel, the
entity id globally); a gap is not recoverable. It should be stated as part
of the contract that clients must be idempotent, not as an accident.

### What a snapshot contains

- **Channel stream:** the channel's history and its members — what
  `GET …/messages` and `GET …/channels/{id}` return today.
- **Global stream:** the sessions with their channels and participants —
  what the sidebar assembles from several calls today.

### How big may a snapshot be

A channel's history grows without bound ([16](16-context-compression.md) is
about the model's context, not this). Sending all of it on every reconnect
is not viable. The likely answer is the last N messages plus a marker that
there is more, with older messages fetched over REST — which keeps the REST
endpoints in use rather than replacing them.

### The global snapshot must be filtered

It may only contain what the logged-in user is allowed to see. Today every
user sees every session, which is [11](11-agent-ownership.md). A snapshot
does not create that problem, but it makes it larger and more visible, and
must go through the same check the REST endpoints will.

### Shape on the wire

SSE named events (`event: snapshot` next to the unnamed data events) keep
existing clients working and let a client register a separate listener. A
query parameter (`?snapshot=true`) decides whether the stream starts with
one, so a client that does not want it does not pay for it.

With snapshots available, a lagging client (`Lagged`) can be answered with a
fresh snapshot instead of a comment: "here is the current state" instead of
"something is missing".

### The alternative: replay by id

`EventSource` sends `Last-Event-ID` on reconnect; a server with a durable
event log could resume exactly where the client stopped. That is more
precise and transfers less, but it needs the log — the `Recorder` of
[05](05-postgres-event-recorder.md), which today is only
`BlackholeRecorder`. A snapshot works without persistence. The two do not
exclude each other; replay can be added later for the common case, with the
snapshot as the fallback.

## Done when

- A client can ask either stream for the current state on connect and
  receives it before the events that follow.
- No event that occurs while a stream is being established is lost;
  duplicates are possible and documented.
- The size limit of a channel snapshot is decided, and older history stays
  reachable.
- A lagging or reconnecting client can recover without reloading the page.

## History

- **2026-09-22** — Issue created out of the discussion of
  [33](33-sse-outlives-its-token.md): ending streams at the token's expiry
  is only acceptable if a reconnect restores the state, and the same gap
  already exists today between loading the history and opening the stream.
