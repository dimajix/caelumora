# 593 — Every change of a lifecycle is guarded in a way of its own

**Status:** open
**Type:** refactoring
**Priority:** high
**Area:** Code structure
**Summary:** A change of an object's lifecycle - a session started or deleted, a participant taken in or let go, a thread opened or closed - is guarded each by a mechanism of its own, built after a race showed, so that every new change of a lifecycle brings new races, found by reasoning in review, hardly by a test, and fixed by yet another mechanism.

## The problem

What may change while what else changes is answered case by case, each time with what the case
needed:

- **A lock per id** serialises the changes of one object: a session's start, stop, deletion,
  restoring and purging (`SessionManager::lifecycles`), a project's (`ProjectManager::lifecycles`),
  a participant's part in a session and in a channel (`SessionLoop::changing`,
  `ChannelLoop::changing`, issues [564](564-taking-someone-in-is-rolled-back-in-part.md) and
  [590](590-a-session-lets-nobody-go.md)), taken in a fixed order - the session's before the
  channel's, several sorted by id. A lock says that one change runs at a time, not in which state
  a change is allowed.
- **A phase** says in which state an object is: a thread founding, open or closing (issue
  [557](557-a-conversation-cannot-go-on-two-ways-at-once.md), step 1 of 2b, not committed yet as
  this is opened). It neither serialises nor waits; whoever changes it advances it from the phase
  it expects.
- **Waiting for a state** is built for each case: a participant added again waits for the task of
  its predecessor (`ChannelLoop::leaving`, issue [171](171-readded-while-still-stopping.md)); a
  thread is closed by `run` only once whoever left it has left, kept meanwhile in a map of its own
  (557).
- **The state of the objects around** is checked by hand at the caller: whether a thread may be
  closed asks the running actors, the attendance of the participants listed without one, and the
  thread's phase - three sources, each a race of its own (557).
- **A lock for one build** keeps an older result from replacing a newer one
  (`ChannelLoop::building_mentions`, issue
  [585](585-a-renamed-participant-is-mentioned-by-its-old-handle.md)), and a document is written
  under one lock alone (issue [492](492-a-document-is-written-under-two-locks.md)).

Each was added after a race had been found: a participant re-added while still stopping (171), a
turn's end lost as its agent was removed
([262](262-stopped-turn-not-recorded-while-reflecting.md)), a session's actors aborted as it
stopped ([377](377-stopping-a-session-aborts-its-actors.md)), a session deleted while it ran
([458](458-deleted-while-it-runs.md)), a lock taken out of its table while held
([495](495-a-lock-per-id-leaves-its-table-while-held.md)), two takings-in undoing each other (564),
a leave crossing a join (590), and in step 1 of 557 a founder taken in while its thread was closed,
the last events of an actor let go refused by a thread closed before it ended, and a close that is
lost when the session stops before the leave it waits for is recorded.

## Why it matters

- **Every new change of a lifecycle brings its races.** Issue
  [558](558-no-agent-hands-a-task-to-a-sub-agent.md) starts, asks and ends sub-agents - ending one
  lets it go, closes its thread and deletes it -, issue
  [381](381-inactive-channels-held-in-memory.md) unloads channels nobody speaks in. Each of them
  would be guarded again by what it needs, beside the mechanisms that are there.
- **The races are found by reasoning, hardly by tests.** A reviewer reads every pair of changes
  against each other; a test provokes one order of them at most, and a mutation of the guard often
  passes it.
- **The fixes add state.** A phase here, a map there, a task tracker kept until someone comes back:
  each is one more thing a reader has to know, with rules of its own of when it is cleaned up.

## The idea

As the maintainer put it: a change of a lifecycle is first only asked for. It then waits until the
object - and the objects around it - are in a state that admits it; then the object is locked
against other changes; the change is checked against the state it finds, and carried out or
refused. Changes of the same object are serialised, and allowed only in certain states.

## Considerations

- **Which changes.** The lifecycle of an object: a session, project or channel started, stopped,
  deleted, restored, purged; a participant taken in, let go, its actor started again or stopped; a
  thread opened and closed; an actor started and ended. Not the ordinary changes of a running
  object - something said, a field of a document - which have their owners already.
- **One owner of an object's lifecycle**, with named states and the changes each admits, instead of
  a lock, a phase and a check at the caller beside each other. Whether that is one generic type
  (a lock per id that knows a state and the transitions allowed from it), or a pattern each owner
  follows.
- **Asked, then carried out by the owner.** The closest the code has: `ChannelLoop::run` applies
  the changes of its actors one after the other (`ActorChange`), and it is where the deferred close
  of a thread already waits. Changes as requests into the object's own loop serialise by
  themselves and can wait without holding a lock; the caller needs an answer back, and a permission
  checked before the request is queued may no longer hold when it is carried out. Against that, a
  lock with a state keeps the calls as they are, and waiting means holding no lock while waiting
  for a state, then taking the lock and checking again.
- **Waiting or refusing.** Which changes wait for a state to come - a thread closed once its
  leaves are recorded, a participant re-added once its predecessor ended - and which are refused
  at once - a thread closed while someone takes part (`ThreadNotEmpty`). A wait for a state that
  never comes needs an end: a timeout, the object stopping, the caller giving up.
- **The objects around.** Session, channel, thread, participant and actor stand in a hierarchy. A
  change of a child needs its parent in a state that admits it - a channel running, its session
  not stopping -; a change of a parent - stopping, deleting - waits for the changes of its children
  under way, or ends them, and admits no new ones. Locks are taken from the top down, in one order,
  as now the session's before the channel's.
- **What survives a restart.** A change asked for and not carried out yet - the close of a thread
  waiting for a leave - is lost when the object stops; whether such a change is kept, or the state
  it waits for is recovered as the object starts again (as a channel reconciles its history, issue
  590).
- **Across processes** a session is held by one process (`SessionLocks`, issue 554); this is about
  the changes within one.
- **Tests.** A pattern of its own can be tested once, for every order of two changes, instead of
  each guard on its own.

## Done when

- The changes of a lifecycle follow one named pattern, decided with the maintainer and described in
  ARCHITECTURE.md and the documents of the objects it covers, and the guards named above are built
  on it, or the reason is named why one stays as it is.

## History

- **2026-11-18** — Opened at the maintainer's request during step 1 of 2b of issue
  [557](557-a-conversation-cannot-go-on-two-ways-at-once.md), whose opening and closing of threads
  needed a phase, a check over three sources and a deferred close to guard against the races its
  review found.
