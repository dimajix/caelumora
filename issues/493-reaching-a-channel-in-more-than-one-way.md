# 493 — What reaches a channel is named, held and asked in more than one way

**Status:** in progress
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** The loops hand out their documents as `Guard`s that every caller clones before an `await`, two functions of different meaning share the name `channel_of`, and the question whether someone reads a channel is put together three times by hand.

## The problem

Three findings of the work on issue [468](468-attachment-manager-takes-a-view.md), all about how
a channel is reached, none of them part of that change.

### The loops lend a `Guard` where their callers need an `Arc`

`ChannelLoop::channel()` (`caelumora-runtime/src/channel/mod.rs:464`) and `SessionLoop::session()`
(`caelumora-runtime/src/session/mod.rs:379`) return `Guard<Arc<_>>`, an `arc_swap` guard. The
rule in [CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md#runtimes-and-their-entities) says a
runtime's getters "return owned values, so that no `arc_swap::Guard` is held across an `await`".
So every caller who hands a document on to an operation - since 468:
`WorkspaceController::terminal_in` and `shell_in`, `AttachmentManager::create` and `copy` - writes
`Arc::clone(&channel_loop.channel())` first; 16 places today. Who forgets, holds the guard across
the operation's `await`, which the compiler does not refuse. `SessionLoop::workspace` and
`ActorRuntime::channel` already answer `Arc<_>`.

### Two `channel_of` that look a channel up differently

`routes::channel_of` (`caelumora-rest-server/src/routes/mod.rs:100`) is the channel as the user
reads it, by its ids: `SessionView::channel`, `ChannelNotFound` otherwise; twelve handlers use it.
`Views::channel_of` (`caelumora-runtime/src/view/mod.rs:419`) is the channel wherever it is - what
a memory entry or a `KnowledgeRef` points to -, answered `Reached`: there, gone with who deleted
it, or never. One name for two lookups, of which the second is the one with a concept of its own.
The REST server's sits beside `open_channel`, which reaches the running channel through
`SessionLoop::channel_for`; by the rule "found by id, by name, for someone" the view for a user
would be a `…_for`, not a `…_of`.

### The question whether someone reads a channel, put together three times

`WorkspaceController::usable_in_channel` (`caelumora-runtime/src/controller/workspace.rs:222`),
`SessionLoop::listed_channel_for` (`caelumora-runtime/src/session/channels.rs:170`) and
`SessionView::reading` (`caelumora-runtime/src/view/session.rs:301`) each build a `Place` from the
channel's id, its classification and the session's privacy, choose a `ChannelReader` and ask
`view::channel_reading`. The `Place` is built the same way three times; the reader is chosen three
ways: by the caller's kind (an agent `From` the channel itself, a user `Cleared`), always `Cleared`
(the REST server's lookup, users alone), and by the viewer's place. Issue 468 nearly added a fourth
copy, in the `AttachmentManager`; it did not, since the manager no longer asks.

## Why it matters

- A rule that only holds where each caller remembers it (the `Arc::clone`) is a rule broken
  sooner or later, with no tool to notice.
- A name that means two things makes the reader look up which one is meant at every use.
- Three hand-made copies of one access question drift apart; the differences in choosing the
  reader may be deliberate, but nothing says so.

## Considerations

- **Guards.** `ChannelLoop::channel()` and `SessionLoop::session()` answer `Arc<_>`
  (`load_full()`), and the `Arc::clone` at their callers goes. Costs one reference count per call
  where a caller only reads a field, which is negligible against what the callers then do. A
  getter that lends - for a hot path - would get a name of its own, but none is known.
- **`channel_of`.** Rename the REST server's to `channel_view_for` (or `channel_for`, beside
  `open_channel`), and keep `Views::channel_of`, whose `Reached` is its concept - or rename that one
  to `reached_channel`. A mechanical rename across `routes/`.
- **The reading question.** At least `Place::of(&Session, &Channel)` in `caelumora-model`, beside
  `SessionPrivacy::of` ("the model derives"), so that the place is built once. Further, one
  function beside `channel_reading` that takes the context, the session and the channel and
  chooses the reader; then the three ways of choosing it have to be either aligned or named with
  their reason - in particular why the workspace treats an agent as reading from the very channel
  it reads, where the views take its own place.

Recommendation: all three as one change, in that order, each a commit of its own; for the third,
`Place::of` first, and the alignment of the readers as a question to the maintainer once the three
are side by side.

## Done when

- No caller of `ChannelLoop::channel()` or `SessionLoop::session()` clones the result to hold it
  across an `await`.
- `channel_of` names one lookup.
- A `Place` of a session's channel is built in one place, and the choice of the reader is either
  one rule or named with its reasons.

## History

- **2026-10-29** — Opened at the maintainer's request from the findings of issue 468.
- **2026-10-29** — The trade-off of the first step, weighed before it was done. Holding a `Guard`
  across an `await` is no error: `Guard<Arc<T>>` is `Send` and correct (arc-swap 1.9.2). It costs
  efficiency only - each thread has 8 slots for guards (`DEBT_SLOT_CNT`); one held long takes one,
  further `load()` on that thread take the slower way, and a writer settles the open slots first.
  So the rule in CODING-GUIDELINES.md was hygiene, not correctness, and the issue's "rule broken
  sooner or later" overstated it. Against `Arc`: `load_full()`, or an `Arc::clone` of the guard,
  raises and lowers the shared reference count on every call, where a guard touches a slot of its
  own thread; callers that only read a field (about ten) pay it too; and an `Arc` invites keeping a
  snapshot - in a field, a long task - where the type `Guard` says "look now". For `Arc`: simpler
  call sites, and an interface that says what the caller gets and nothing of how the loop keeps
  it. The public getters are no hot paths - the loop reads itself through `self.channel.load()` -,
  so the cost does not count. Decided by the maintainer for `Arc`, the clean interface being the
  main argument; the getters' doc comments say the document is a snapshot, not to be held.
- **2026-10-29** — Step 1 done: `ChannelLoop::channel()` and `SessionLoop::session()` answer
  `Arc<_>`; the `Arc::clone` at their callers is gone, and the reason in CODING-GUIDELINES.md
  ("Runtimes and their entities") says what holds.
