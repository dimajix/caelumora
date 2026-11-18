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

## Proposal: procedures

The maintainer's proposal of 2026-11-18, made as "transactions"; the name is open (see *Open
questions*), *procedure* is the working name here:

- **A procedure orchestrates one change of the logical object graph** - a change of a lifecycle as
  above - across the objects it touches, a type of its own. It is first queued with an executor.
- **It locks what it enumerates.** It finds what it would change from the top down - a session
  before its channels, a channel before its threads and participants - and locks each object as it
  finds it: exclusively what it changes, shared the parents that only have to stay as they are. An
  object it finds locked defers it: it lets go of everything it holds and waits until that object
  is free, then enumerates again, since the procedure that held the object may have changed what
  there is. Enumerating reads and locks, nothing else.
- **Holding its objects, it checks the state it finds** - permissions too, as they depend on it -
  and carries the change out or refuses it. Where an object's own loop carries a step out - a
  channel's `run` stopping an actor -, the procedure asks it and waits for its answer; where actors
  have to come to rest, it tells them and waits for them, within a grace, as `STOP_GRACE` bounds it
  now. It holds its objects until the change is complete, not only until it was handed on.
- **It keeps a compensation log** of what it did as it goes, and undoes it if a step fails.
- **It ends carried out, refused or failed**; its objects are free again, and the procedures
  deferred on them go on.
- **Idempotent and serialisable**, so that the executor could keep the procedures queued and under
  way and go on with them after a restart.

### Assessment

How the races named above fare:

- **A change against a change** - a session deleted while it runs (458), two takings-in undoing
  each other (564), a leave crossing a join (590), a founder taken in while its thread is closed
  (557) - is ruled out by the conflict on an object alone.
- **A change against the ordinary flow of `run`** - a participant re-added while its old actor
  still stops (171), the last events of an actor let go refused by a thread closed before it ended
  (557) - only because a procedure holds its objects until `run` has carried the change out. Then
  what says that a change is under way goes: a thread's `Phase::Founding` and `Phase::Closing`, the
  closes `run` keeps waiting (`closing`), `ChannelLoop::leaving` and the wait for it (557, step 1 of
  2b, not on `main` as this is written).
- **A close lost as the session stops** before the leave it waits for is recorded (557): the stop
  locks the session exclusively and waits for the procedures that hold it shared.
- **An actor's own work against a change** - a turn's end lost as its agent was removed (262), a
  session's actors aborted as it stopped (377) - stays the actor's: a procedure tells it to come to
  rest and waits; how it keeps what it is in the middle of is not the executor's.
- **A lock taken out of its table while held** (495) moves into the executor, which keeps the table
  of the objects locked, and is solved there once.

What it replaces besides:

- The fixed order of the locks - the session's before the channel's, several sorted by id
  (`SessionLoop::restore_channel`) -, since nothing is waited for while anything is held.
- The rollbacks spread over the objects - `SessionLoop::roll_back_taking_in`, the controllers'
  `roll_back_participant_addition`, `ChannelLoop::roll_back_opening` (557), the purge in
  `SessionManager::persist`, `SessionManager::roll_back_creation` -, which become the compensation
  logs of procedures.
- Changes run in the caller's task, where a caller giving up - a request dropped - ends a change
  midway without its rollback.

What it costs: every change of a lifecycle in `SessionManager`, `ProjectManager`, `SessionLoop` and
`ChannelLoop` is written anew as a procedure; a key over objects of different kinds; the executor,
with shared and exclusive locks, its queue and procedures given up; `run` answering the steps it is
asked for. In steps: the executor in place of `LocksById` first, without a change of behaviour.

A compensation log is no rollback: an event announced or an actor started can be balanced, not
taken back, and what is no procedure - `run`, a reader through the REST API - sees the states in
between. Hence not "transaction", which promises an isolation there is not.

## Decision

The maintainer's decisions of 2026-11-18 on the assessment:

- **Changes of a lifecycle only.** A procedure is for the changes of the logical object graph,
  where most conflicts arise; a plain change of an attribute - a rename - is not one.
- **Locked as enumerated, deferred on a conflict**, and enumerated anew when it goes on, as above.
- **Fair.** A key at which a deferred procedure waits admits nobody after it, shared neither, so
  that a stop is not starved by a stream of small changes.
- **No procedure within another.** Code shared between procedures is welcome; a procedure starting
  another is not - it would wait for itself.
- **Run in a task of the executor's**, not the caller's: a caller giving up does not cut a change
  off midway.
- **The compensation logs move into the procedures**, together with the logic of the rollbacks
  named above.
- **The objects are simple, the procedures orchestrate.** The methods of a lifecycle leave the
  objects - session, channel, actor - entirely, instead of staying as methods that call the
  executor: that would be a second way to the same thing, and one object acting on another's
  behalf. The objects know nothing of locking, not even indirectly; they keep their building blocks
  and the invariants of the single object - a participant listed once, the line never closed. The
  procedures hold the order, the checks of state and permission across objects, and the
  compensation: the complexity moves into them. A channel keeps its `run` for the ordinary flow;
  its building blocks for running actors become requests to `run` that answer.

## Open questions

- **The name.** "Transaction" promises an isolation there is not; "operation" is so general that
  a rename is one too, and [PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md) uses it for any
  action already; `LifecycleManager` is narrower than meant. Proposed: *procedure* - steps in an
  order, with compensation -, carried out by a `ProcedureExecutor`, as the `ToolExecutor` carries
  out tools; a `…Manager` is a module's manager
  ([CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md#a-modules-manager)).
- **No building block past a procedure.** Visibility cannot say it in Rust: `pub(in …)` names only
  enclosing modules, and a `pub(crate)` building block of `session` can be called from all of the
  kernel. As a convention, it would be held by review again - which this issue is to end. Proposed:
  a building block that changes an object asks for a proof of admission that only the executor
  gives out; the object neither locks nor checks, it only asks for the proof. It departs from "not
  even indirectly" above.
- **Persistence.** Recommended not to take it up for now: what runs - loops, actor tasks, the
  answer to the caller - does not survive a restart; a procedure gone on with acts in a
  `SecurityContext` whose rights may be gone meanwhile; which process goes on with it would be bound
  to `SessionLocks` (554); and the pattern for a restart is the reconciliation as an object starts
  again (590). A user it may have: a crash in `SessionManager::persist` after the session was
  stored and before its group, its project and its channels - whether anything clears up what is
  left then is not checked yet.
- **Where the procedures lie.** Proposed: a module `procedure` with a module for each object, named
  by the topmost object a procedure locks, so that what belongs to a subject lies together while a
  procedure touches several objects.
- **The interface** of the procedure and the executor, put before the maintainer before any body is
  written.

## Done when

- The changes of a lifecycle follow one named pattern, decided with the maintainer and described in
  ARCHITECTURE.md and the documents of the objects it covers, and the guards named above are built
  on it, or the reason is named why one stays as it is.

## History

- **2026-11-18** — Opened at the maintainer's request during step 1 of 2b of issue
  [557](557-a-conversation-cannot-go-on-two-ways-at-once.md), whose opening and closing of threads
  needed a phase, a check over three sources and a deferred close to guard against the races its
  review found.
- **2026-11-18** — The maintainer proposed procedures, as "transactions"; assessed, and decided as
  *Proposal: procedures* and *Decision* record. Open: the name, a proof of admission for the
  building blocks, persistence, where the procedures lie, and their interface.
