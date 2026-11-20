# 593 — Every change of a lifecycle is guarded in a way of its own

**Status:** in progress
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

## Decision on the open questions

The maintainer's decisions of 2026-11-18 on the open questions above, and on the conflicts with
written rules the plan for them found:

- **The name.** *Procedure*, carried out by the `ProcedureExecutor`. Where the documents call a
  skill "procedures written down" - ARCHITECTURE.md, STORAGE.md, the module docs of
  `caelumora-skills` -, they are reworded, so that the word has one meaning.
- **The key is the path.** `Session(SessionId)`, `Channel(SessionId, ChannelId)`,
  `Thread(SessionId, ChannelId, ThreadId)`, `Participant(SessionId, ActorId)` and
  `Project(ProjectId)`. The ids alone would identify - each is a UUID, and a channel's line takes
  its channel's -, but that a procedure locks the parents of what it changes shared would then be a
  convention, held by review. With the path, the executor locks the parents of a key shared itself:
  a session or a channel locked exclusively conflicts with everything below it. A participant hangs
  under its session alone, as it takes part in the session and in every channel of it - one key in
  place of the two tables `SessionLoop::changing` and `ChannelLoop::changing` -; who takes it into a
  channel locks the channel besides. A project is no part of the path, since a session is put into
  a shared project and taken out again: a procedure locks it where it matters.
- **Procedures are public types**, which the entry points make - a REST handler, the terminal, a
  plugin's writer, whoever follows an event - and hand to the executor. The managers and the loops
  are building blocks, and their methods of a lifecycle go; a new procedure changes no manager, and
  a plugin writes procedures of its own (the end of a sub-agent, `SubAgents::wind_up`, issue
  [558](558-no-agent-hands-a-task-to-a-sub-agent.md), is one). It changes what
  CODING-GUIDELINES.md says of a module's manager, the one owner of a kind's lifecycle, and
  *Managers and controllers* of ARCHITECTURE.md.
- **Out of scope: the managers of a kind** - `AgentManager`, `GroupManager`, `WorkspaceManager`, the
  plugins' -, whose lifecycles change one object: building blocks alone. The lifecycles of sessions
  and projects are in scope, with every verb of them, a session's creation too; a project's creation
  is no procedure, since nothing anybody else reaches is locked or changed by it.
- **Permissions are checked once, under the locks**, first in carrying a procedure out, before
  anything is asked of others - of `run` too. Enumerating reads in the kernel's name and shows the
  caller nothing.
- **Errors.** `Procedure` names its error type (`type Error`), which takes in what the executor
  fails with (`From<ProcedureError>`: nested, shutting down, ended without an answer); the executor
  answers it as it is. Each procedure translates what its building blocks fail with.
- **The proof of admission** is `Admitted`, made by the executor alone and bound to the procedure's
  hold by its lifetime, so that no task keeps it. It proves that a building block is called in
  carrying a procedure out, not which objects that procedure locked: that would be a check in the
  object. Which objects a procedure locks, its tests hold.
- **Persistence: not now**, as recommended above; and no bound of idempotence or serialisation on a
  procedure while nothing uses it.
- **Where they lie.** A module `procedure` of `caelumora-kernel`, with a module for each object
  whose lifecycle a procedure changes - the one it locks exclusively: `session`, `project`,
  `channel`, `participant`, `thread`. Named by the topmost object it locks, nearly every procedure
  would lie in `session`, which almost all lock shared. A plugin's procedures lie in the plugin.
- **Shutting down within its bound.** A stop that waits for procedures holding the session shared,
  which wait up to `LEAVE_GRACE` themselves, and then `STOP_GRACE` for its own actors, would exceed
  `SHUTDOWN_BOUND`: the executor admits no procedure any more, waits for those under way and stops
  the sessions, all within the bound; what is still under way then ends with the process, as now.
- **Opening a session is one procedure, `session::Open`, whether it runs or not**: enumerating
  locks the session shared for one that runs - which keeps it running while the channels left out
  are built again -, and exclusively for one it starts - so that no entry point looks first whether
  it runs. Building a channel left out as its session started is a
  procedure of its own (`channel::Start`, for `SessionLoop::retry_unavailable`), with the session
  locked shared - as `SessionManager::open` now lets go of its lock before it builds them.
- **Enumerating fails at nothing but a conflict** (`Deferred`): what reading fails at goes into what
  it found, and carrying the procedure out answers it - one way for an error to reach the caller.
- **A session being created is held until it is complete.** `SessionManager::create_from_template`
  completes the session - its workspace, its participants, its project - after `establish` let go
  of its lock, while it runs and is found already, so that stopping or deleting it meanwhile
  crosses the completion and its rollback. `session::Create` chooses the session's id before it is
  built and holds it exclusively until it is complete or rolled back.
- **`run` answers.** Every request to `run` (`ActorChange`) carries what `run` answers once it has
  carried it out: a leave once the actor's task has ended and its leave is recorded, a close once
  the thread is closed and announced. A request dropped unanswered means that the channel stopped,
  its actors with it: a leave counts as done.
- **No procedure within another is checked at run time.** The executor marks the task it carries a
  procedure out in (a task-local); a procedure asked of the executor from such a task is refused at
  once (`ProcedureError::Nested`) and logged - it would wait for whoever holds the keys it needs,
  the procedure that asked. Only a task a procedure spawns escapes the check; the executor is held
  by the entry points alone, by no building block.
- **What a procedure waits for gives up waiting for a procedure** once it is let go or stopped. An
  actor does: leaving interrupts the tool call it is in, and the procedure the call asked for goes
  on in its own task, deferred and holding nothing, until the procedure that let the actor go is
  done; one that does not react is aborted after `LEAVE_GRACE`. It is not detected: that would need
  the executor to know whom a procedure waits for and from whose task another was asked, which the
  caller of its context does not say. The executor logs a procedure that waits at a key longer than
  `LEAVE_GRACE`, with the key and the procedure that holds it.

Beside them, decided on the same day:

- **Moving a session is a procedure** (`session::Move`): put into a shared project, or taken out
  (`SessionLoop::update_project`). Of itself an attribute; but deleting a project enumerates its
  sessions, and a session moved into it meanwhile would cross that. It locks the session
  exclusively and the project it leaves and the one it goes to shared; `SessionLoop::moving`, a
  lock of its own not named above, goes.
- **Reclassifying a channel is a procedure** (`channel::Reclassify`): raising its classification
  asks whether every participant is cleared for it (`ChannelError::NotCleared`), and taking someone
  in asks whether they are cleared for the classification - side by side, one not cleared could get
  in. It locks the channel exclusively.
- **Who follows the events is an entry point.** The loop of a session follows the agents deleted
  and restored, and all of them after events it lost (`SessionLoop::follow_agents`); with
  procedures, it would hold the executor, and an object would know of locking. Following moves to
  a follower of the kernel, which holds the executor and has the agents followed by a procedure
  (`participant::Follow`); the session's loop keeps following renames, which change no lifecycle.
- **Taking someone into a stopped session starts it**, as `SessionManager::join` now opens it in
  the kernel's name: enumerating locks the session exclusively where it does not run, and starting
  it is code `session::Open` shares - not two procedures one after the other at the entry point,
  between which the session could stop again.

The procedures, by step, each step reviewed and committed on its own:

1. The executor, with the lifecycles of sessions and projects, in place of
   `SessionManager::lifecycles` and `ProjectManager::lifecycles`: `session::Create` (from a
   template; `roll_back_creation` becomes code it shares with deleting and purging), `Open`,
   `Stop`, `Delete`, `Restore`, `Purge`, `ShutDown`, `Move`; `project::Delete`, `Restore`,
   `Purge` (the deletion of a project's sessions code it shares with `session::Delete`, in place
   of `SessionManager::delete_with`).
2. `run` answers its requests, without a change of behaviour; then, as a step between it and the
   next, `run` takes the receiver of its requests, so that what it did not carry out goes with it,
   and a leave it finishes as an actor starts under the same id closes the threads that waited for
   it; then, as a second step between, a channel no longer stops by itself once its last actor
   ended, but runs on, empty, until its session stops or it is deleted.
3. Participants, in two steps: first `participant::TakeIn` (added or joining, into the session and
   a channel of it) and `LetGo` (removed or leaving, from the session with every channel of it, or
   from one channel), in place of `SessionManager::join`, `ChannelLoop::leaving` and
   `ChannelError::StillLeaving`; then `Follow` (agents deleted or restored), with the follower of
   the events. `changing` of both loops and the rollbacks of taking in go with the last way in that
   is no procedure yet - founding a channel or a thread, steps 4 and 5 -, as the maintainer
   decided: until then they keep those ways and the procedures apart. Taking founders in is code
   that creating a session or a channel and opening a thread share.
4. Channels: `channel::Create` (with its founders), `Delete`, `Restore` (letting go whom the
   session let go meanwhile), `Start` (one left out as its session started), `Reclassify`; the
   order of locks in `SessionLoop::restore_channel` goes.
5. Threads and sub-agents: `thread::Open` (with its founders) and `Close`; in
   `caelumora-subagents`, starting a sub-agent - an agent made, a thread opened, a brief delegated -
   and ending it - let go, its thread closed, the agent deleted -, in place of its `apart` and its
   rollback; `Phase` and the threads `run` keeps closing go.
5b. Taking in and letting go, as the procedures orchestrate them: once no way in is left that is no
   procedure, `ChannelLoop::take_in` and `SessionLoop::take_in`, `add_participant_held` and
   `join_held` are taken apart, and so are `remove_participant_held`, `leave_held` and the session's
   `let_go`, which goes through every channel and writes the documents of those that do not run
   (`let_go_where_unavailable`). The checks of state and permission are asked once under the locks,
   each by the object it concerns, in the order the procedures call them; one function of the module
   `procedure` takes someone in, shared by
   `participant::TakeIn`, `channel::Create`, `session::Create` and `thread::Open`; the loops keep
   the building blocks - resolving, storing and its undo, starting and ending the actor - and the
   invariants of the single object. `changing` of both loops goes, and with it `LocksById`, and the
   rollbacks of taking in become compensations.
5c. Thin building blocks: what the building blocks of channels, sessions and projects still
   orchestrate across objects, check across objects and roll back moves into their procedures, the
   rollbacks becoming compensations - in three steps, one per object:
   - channels: `create_channel_held` (`roll_back_channel_creation`), `delete_channel_held`
     (`roll_back_channel_deletion`), `restore_channel_held` (its rollback within it);
   - sessions: `make_from_template` with `complete`, `establish` and `persist` (the purge named
     above), `delete_held` (`roll_back_deletion`), `restore_held` (`restore_kept`, marking again),
     `SessionLoop::update_project` (`roll_back_project_change`);
   - projects: the deletion, restoring and purge of a shared project's sessions, which the
     `ProjectManager`'s building blocks orchestrate, move into `procedure::project`.
5d. Lifting a session's own project into a shared one (`ProjectController::lift`) is a procedure,
   `project::Lift`: it changes whether the project goes with its session's deletion, restoring and
   purge, and whether a project's deletion reaches it - as moving a session does (`session::Move`).
5e. Actors end through `run`: stopping and letting go every actor of a channel
   (`ChannelLoop::stop_actors`, `leave_actors`, as a session stops, a channel or a session is
   deleted, a channel is taken back) become requests to `run` that answer, as a single leave is,
   in place of calling the actors and waiting for their tasks beside `run`.
6. The documents: ARCHITECTURE.md, CHANNELS.md, CODING-GUIDELINES.md, PERMISSION-CHECKS.md.

No procedure: the attributes their controllers write - of a session and a channel their name,
language, time zone, privacy, description, rules of approval, whether a channel uses the workspace,
and which workspace a session has, which is a kind of a manager's -; the ordinary flow of `run` -
what is said, cancelling, waking, interrupting a tool call, delegating into an open thread (`run`
refuses what is delegated into a thread closed), an actor that ends by itself, the internal actors
`run` starts and lets go -; the lifecycles of the kinds of managers, whose effect on running
channels a procedure follows (`participant::Follow`); and `ChannelLoop::building_mentions` and
writing a document under one lock (issue [492](492-a-document-is-written-under-two-locks.md)):
neither guards a lifecycle.

## Later: a channel that rests

Not built, recorded at the maintainer's request. A channel runs on, empty, since whether it stops
has to be decided under the locks of a procedure: one that stopped by itself as its last actor
ended stopped outside of any, and `participant::TakeIn`, holding the channel, could find it running
and hand a start to a loop that ends just after - read as stopped, while the session still holds
the channel as running. Built on that, an empty channel could rest: the session holds it as stopped,
without its loop - also one empty as the session opens -, `participant::LetGo` stops it as it lets
the last one go, and `TakeIn` starts it again as it takes someone into it, each undone by its
compensation. It saves a waiting task for every empty channel, at the cost of a state of the
session's, and of starting and stopping in two procedures. Issue
[381](381-inactive-channels-held-in-memory.md) unloads an idle channel and starts it again when it
is needed - the session keeping its entry without its loop -, of which an empty channel is one
case; it is decided there, and may make this unnecessary.

## Done when

- The changes of a lifecycle follow one named pattern, decided with the maintainer and described in
  ARCHITECTURE.md and the documents of the objects it covers, and the guards named above are built
  on it, or the reason is named why one stays as it is - among them the check after writing of
  `ScopeHolders` (issue 458), by which the managers of a kind find a session, channel or project
  deleted meanwhile, and the holds across processes (`SessionManager::holds`, issue 554).

## History

- **2026-11-18** — Opened at the maintainer's request during step 1 of 2b of issue
  [557](557-a-conversation-cannot-go-on-two-ways-at-once.md), whose opening and closing of threads
  needed a phase, a check over three sources and a deferred close to guard against the races its
  review found.
- **2026-11-18** — The maintainer proposed procedures, as "transactions"; assessed, and decided as
  *Proposal: procedures* and *Decision* record. Open: the name, a proof of admission for the
  building blocks, persistence, where the procedures lie, and their interface.
- **2026-11-18** — The open questions decided, with the conflicts with written rules the plan for
  them found, as *Decision on the open questions* records; begun on the branch
  `issue-593-procedures`. Since this issue was opened, step 1 of 2b of issue 557 is on `main`
  (`Phase`, the threads `run` keeps closing, `ChannelLoop::leaving`), and issue 558 is done: its
  writer of sub-agents composes the first change of a lifecycle a plugin makes, letting an agent
  go, closing its thread and deleting it (`SubAgents::wind_up`).
- **2026-11-18** — Step 1 implemented, not committed yet: the module `procedure` of the kernel, with
  the executor, the keys and their locks, the proof of admission and the compensation, tested for
  every order of two procedures; the lifecycles of sessions and shared projects as procedures, in
  place of `SessionManager::lifecycles`, `ProjectManager::lifecycles`, `SessionLoop::moving` and
  the flag `SessionManager::shutting_down`; the REST server, the terminal and the server program
  make procedures. Decided in implementing it: the executor is the kernel's, one per process
  (`Kernel::procedures`), which the entry points take once and the plugins reach in step 5;
  deleting, restoring and purging a session lock the session alone, since the procedures of a
  project lock every session in it; starting the sessions interrupted is no procedure but runs
  `session::Open` for each, as an entry point; the rollbacks within deleting, restoring and storing
  a session stay in their building blocks, the creation's moved into the compensation. A session
  being created is held until it is complete, which closes the gap after `establish`.
- **2026-11-18** — Reviewed with fresh eyes; what it found in the change is taken in. Deleting,
  restoring and purging a session lock the project it lies in shared after all, as every way into
  a project or out of it does - creating a session in it, moving one into it -, so that the
  procedures of a project, which list its sessions again as they carry the change out, find those
  they locked; a session restored while its project is deleted crossed it before. Moving a session
  locks only the project it goes to: deleting the one it leaves locks the session, and enumerates
  anew once it is moved. Opening and stopping a session are building blocks of the
  `SessionManager` of their own (`open_held`, `stop_held`), and only a running session has the
  channels left out built again. The proof of admission is made in the module of the locks
  alone, out of reach of the modules of procedures. Since every request that reaches a session is
  a procedure, a shutdown refuses reading a running session too (`503 shutting_down`), no longer
  only starting one; and a procedure under way that takes long - an opening that builds channels
  again - holds back every stop of the shutdown, within `SHUTDOWN_BOUND`. A procedure keeps its
  place in the queue of one key alone, fair for each key and not across keys; one that panics is
  not undone.
- **2026-11-18** — Step 1 committed. Step 2 implemented, not committed yet: every change handed to
  `run` goes through `ChannelLoop::request` and is answered once `run` carried it out (`Answer`) -
  a start once the actor runs and its join is announced, a leave once its task has ended and its
  leave is announced, an opening once announced, a closing once the thread is closed, a brief once
  handed to the delegate -, dropped with `run` where it is still under way. Nobody reads the
  answers before the later steps, as the maintainer decided against giving the leave alone its
  answer with its one reader of today (`ChannelLoop::left`); `Answer` says so by an expectation of
  `dead_code`. A leave asked for again while the actor is leaving - one let go while it rests -
  is announced once and answered to both; a brief dropped for a thread no longer open counts as
  carried out.
- **2026-11-19** — Step 2 committed. The step between it and step 3, from the review of step 2,
  at the maintainer's request, implemented, not committed yet: `run` takes the receiver of its
  requests as it starts, in place of `ChannelLoop::run_lock` - it runs once, and the requests
  still queued as it ends, or handed in after, are answered with nothing, so that a reader can rely
  on a missing answer meaning that the channel stopped; and the leave of an actor `run` finishes
  as another starts under its id now closes the threads that waited for it, as the end of its task
  does (`finish_leave`) - before, such a thread stayed open, and its closing unanswered, until
  another left it.
- **2026-11-19** — The step between committed. A second step between 2 and 3, at the maintainer's
  decision, implemented, not committed yet: a channel no longer stops by itself once the task of
  its last actor ended - `TakeIn` could not rely on what it finds under its locks if a channel
  stopped outside of any. `run` runs until its task is aborted, as its session stops or the channel
  is deleted (`run` returns `Result<Infallible>`, and logs its stop as its task ends), and an
  empty channel runs on, for whoever comes in; before, someone added to a channel that had become
  empty was stored, but their actor never started. So `channel::STOPPED` is announced as a channel
  is deleted, no longer as its last actor ended, which a session's stop raced. Resting empty
  channels, as the maintainer asked, are recorded as *Later: a channel that rests*, to be decided
  with issue [381](381-inactive-channels-held-in-memory.md).
- **2026-11-19** — The second step between committed. From its review, at the maintainer's
  decision, not committed yet: a session's stop announces every channel it started stopped
  (`SessionLoop::announce_channels_stopped`), once the session's task has ended and before the
  session, so that `channel::STARTED` and `STOPPED` pair again - taking their tasks, so that a
  channel deleted meanwhile is not announced stopped twice; and `SessionLoop::run` returns
  `Result<Infallible>`, as `ChannelLoop::run` does.
- **2026-11-19** — Committed. At the maintainer's decision documented rather than handled: a
  channel that panics ends its session's loop with every channel, while the session stays
  registered as running until it is stopped (`SessionLoop::run`, CHANNELS.md).
- **2026-11-19** — Step 3a implemented, not committed yet, as the maintainer decided on its
  interface: `procedure::participant::TakeIn` (`added`, `joining`) and `LetGo` (`removed`,
  `leaving`, from the session or with a channel named from it alone), each holding the participant
  (`Key::Participant`) and the channels it changes until `run` has carried the change out, opening
  a stopped session as it starts - one taking someone in by their own act in the kernel's name for
  who may join or read it (`SessionManager::open_to_take_in`, in place of `SessionManager::join`),
  the others as opening a session does (`lock_to_open`, shared with `session::Open`). The building
  blocks are the loops' `…_held` (`add_participant_held`, `join_held`, `remove_participant_held`,
  `leave_held` of the `SessionLoop`; `remove_participant_held`, `leave_held` of the
  `ChannelLoop`). The REST server, the commands `/participant add`, `/participant remove`,
  `/channel join` and `/channel leave` (`CommandContext::carry_out`) and the sub-agents' end make
  the procedures; who reaches a session from within it finds the executor and the session manager
  there (`SessionLoop::procedures`, `sessions`), the kernel knowing the latter weakly
  (`Kernel::sessions`, issue 601). `run`'s answers are read now: taking someone in waits for the
  start, letting go for the leave, at most `LEAVE_GRACE` - in place of `ChannelLoop::leaving` and
  `left`'s wait for the old actor's task, and of `ChannelError::StillLeaving`, whose answer
  `409 still_leaving` is gone; a channel that stops before it started a newcomer answers
  `ChannelError::Stopped` (`409 channel_stopped`). Taking in waits for the start without
  `changing`, which the session's following of its agents takes in the loop that starts the
  channels: waiting under it locked the two; a founder does not wait, founding being no procedure
  yet. Letting go waits for the leave holding the channel's `changing`, so that founding a thread,
  no procedure yet, does not take in one whose old actor still ends; it waits only where `run` has
  started - before, no actor had a task -, so that the following of the agents, which waits as
  well until step 3b moves it out of the session's loop, never waits for a channel the loop has
  not started. A channel taken out of its session before it started drops its requests
  (`ChannelLoop::never_to_run`), so that nobody waits for its answer for good. Letting go from the
  session lets go from its running channels all at once, and locks every channel of the session.
  As the maintainer decided, `changing` stays until founding is a procedure, and with it the
  rollbacks of taking in within the building blocks: in a compensation they would run after the
  procedure's `changing` was let go. Taking someone in by `TakeIn::added` opens the session as
  reading it, as the REST server did before; who left a session by their own act and was not let
  go from every channel of it no longer reaches it, and whoever may change it finishes it.
- **2026-11-19** — Step 3a committed. Step 3b implemented, not committed yet: the agents deleted
  and restored are followed by `procedure::participant::Follow` - `agent`, in every running
  session whose channels list it, or `session`, every agent of one -, holding the agent's part in
  each session (`Key::Participant`) and its channels shared, by the building block
  `SessionLoop::follow_agents_held`; the kernel's follower (`AgentFollower`), which the server
  program, the terminal and the tests' `World` start, carries one out for every deletion and
  restore, each in a task of its own so that it misses no event while one waits, and one for every
  running session after it lost events. The session's loop follows renames alone. A session
  follows its agents as it starts, in `SessionManager::open_held`, holding it exclusively: the
  follower does not find it running before it is registered, and an agent deleted while its
  channels were built would be missed. From the review: the follower is an entry point of its own
  (`procedure::follower`), beside the procedures; `Follow` holds an agent in every running session,
  not only where it is listed, so that one taken in meanwhile is followed after the taking in, and
  following a session follows the agents it holds there alone; a restored agent's actor is waited
  for until `run` started it, as taking in waits.
- **2026-11-19** — Step 3b committed. Step 4 implemented, not committed yet, as the maintainer
  decided on its interface: `procedure::channel::Create` (its id chosen before it is built - so
  `Channel::build` takes it -, holding it and its founders until they joined; taken back by its
  compensation, `SessionLoop::abandon_channel_held`, which now announces a channel it started as
  stopped), `Delete`, `Restore` (holding the deleted channel's participants), `Start::left_out`
  (holding the channels left out, opening no session) and `Reclassify`, each holding the channel
  exclusively, all but `Start` opening the session as opening does;
  taking in and letting go hold it shared, so that neither crosses them. The loops' verbs become
  building blocks (`create_channel_held`, `delete_channel_held`, `restore_channel_held`,
  `retry_unavailable_held`, `ChannelLoop::reclassify_held`). Building again the channels left out
  is a procedure of its own, which whoever opens a running session carries out after `Open` - the
  REST server's `ensure_session` -, so that `Open` no longer holds the session meanwhile; the lock
  `SessionLoop::rebuilding` goes, since `LetGo` and `Start` hold the channels. The REST server and
  the commands `/channel create` and `/channel delete` make the procedures; reclassifying reaches
  the channel as the REST server did (`channel_for`). `changing` and its `lock_all` in restoring
  stay until step 5.
- **2026-11-19** — Step 5b added to the steps at the maintainer's request: `ChannelLoop::take_in`
  and the session's taking in still check, lock and roll back within the building blocks, which
  the steps so far left as they were while `open_thread` takes founders in without a procedure.
- **2026-11-19** — Step 4 committed. From a survey of what is left outside the procedures, at the
  maintainer's request, the steps extended: 5b covers letting go as well; 5c makes the building
  blocks of channels, sessions and projects thin, their rollbacks compensations - including those
  step 1 left in the building blocks on purpose -; 5d makes lifting a session's own project a
  procedure; 5e has the actors of a channel stopped and let go through `run`.
- **2026-11-19** — Decided by the maintainer, from the review of step 4: building again the
  channels left out waits where `SessionLoop::rebuilding` skipped, and runs also right after an
  opening that started the session. Requests at the same time build a channel that stays
  unavailable again one after the other, and a start tries it once more at once - rare and cheap,
  and who asks after its bundle was shared again finds it built, where a skip could answer
  `ChannelUnavailable` while another build was about to succeed. Recorded in CHANNELS.md.
- **2026-11-19** — Step 5 implemented, not committed yet, as the maintainer decided:
  `procedure::thread::Open` (the new thread's id chosen before it is opened, holding it, the thread
  it is opened at and its founders) and `Close` (holding the thread until `run` closed it); the
  loops' verbs become building blocks (`SessionLoop::open_thread_held`,
  `ChannelLoop::open_thread_held`, `close_thread_held`). `LetGo` and `Follow` hold the thread the
  one they let go takes part in shared, until they have left, so that a thread is closed only once
  whoever left it has left: `run` closes a thread at once and answers then, and `Phase` -
  `Founding`, `Closing` - and the closes `run` kept waiting (`closing`, `close_settled`,
  `finish_leave`) go. The sub-agents' start and end are procedures of their own, of the kernel's
  building blocks (`Starting`, `Ending` in `caelumora-subagents`): the agent made and its thread
  opened are undone by their compensation, the last first, in place of `roll_back_start`, and
  `apart` goes, the executor carrying each out in a task of its own. The agent `Starting` makes is
  held by no key, since nobody knows it before it is made. `changing` and the founders' taking in
  stay until step 5b. From the review: `Follow` holds also the thread a resting agent would rejoin
  as it is restored (`ChannelLoop::thread_taken_part_in`), since without `Phase::Closing` a restore
  could start its actor in a thread being closed; the plugin's procedures answer `SubAgentError`,
  into which it translates what the executor fails with, and read the sub-agent in the kernel's
  name as they enumerate, asking again under the locks. An actor that does not end even when
  aborted has letting it go give up after `LEAVE_GRACE`; its late leave may then follow a closing,
  which is logged rather than handled.
- **2026-11-19** — Step 5 committed. At the maintainer's request, what letting a participant go
  locks is one public function, `procedure::participant::lock_letting_go`, which `LetGo` and the
  sub-agents' `Ending` share: the latter rebuilt it by hand, and without the threads the one let go
  takes part in in other channels.
- **2026-11-19** — Step 5b split at the maintainer's request: first taking in, then letting go with
  `changing` and `LocksById`. The first part implemented, not committed yet: one function takes
  someone in, `procedure::participant::take_in`, shared by `TakeIn`, `channel::Create`, a template's
  founders and `SessionManager::make`, with the way in as `Entry` (`Added`, `Joining`, `Founding`,
  `FoundingThread`). It has two parts: into the session (`take_into_session`), where everything
  asked of the one taken in is asked once (`SessionLoop::take_in_held`), and into the channel
  (`take_into_channel`); `procedure::thread::open`, which `thread::Open` and the sub-agents'
  `Starting` share, asks every founder into the session before the thread is there, then opens it
  and takes each into the channel, in the order named. The channel's second asking of joining,
  whether one may run and is cleared goes. The building blocks list and take back a listing
  (`SessionLoop::undo_listing_held`; `ChannelLoop::list_participant_held`, `undo_listing_held`)
  and open a thread without founders (`ChannelLoop::open_thread_held`, `undo_opening_held`); the
  rollbacks - `roll_back_taking_in`, the channel's of a failed start, `roll_back_opening` - become
  compensations. A founder of a thread taken back is now taken out as if never taken in, rather
  than let go as a former participant. `ChannelLoop::add_participant`, `join`, `found`, `take_in`
  and the session's `add_participant_held`, `join_held`, `found` go; the tests of threads move from
  `session::threads` to `procedure::thread`. At the maintainer's request, the procedure starts the
  actor by a building block with the proof as well (`ChannelLoop::start_participant_held`); the
  starts within the channel - as it is built, as it follows its agents - stay without it until
  step 5e.
- **2026-11-19** — Step 5b-1 committed. Then, at the maintainer's request and as a step of its own
  before 5b-2, without a change of behaviour: each object asks its own of the one taken in, as the
  building blocks of the other lifecycles check themselves. `SessionLoop::take_in_held` is taken
  apart into `newcomer` - whose participant they are, as the session asks it, read and resolved,
  storing nothing - and `list_participant_held`; the channel asks its own entry
  (`ChannelLoop::require_entry`: `JOIN`, or a thread's founder the opener's own) and the clearance
  for it (`ChannelLoop::require_admissible`, which `SessionLoop::require_to_take_in` asks it). The
  procedure calls them in the order the security suites hold - whose participant first, the
  session's then the channel's, then anything in their name (issue 562) -, and lists them in the
  channel as resolved once (`ChannelLoop::list_participant_held`), so that
  `ChannelError::ParticipantGone`, which nothing raises any more, goes. `UpdateSession` and
  `UpdateChannel` stay asked by `TakeIn` before anything of the one taken in, as `require_changing`
  of each object. The session follows the one taken in as it reads them, before resolving
  (`newcomer`), so that its actor follows a deletion meanwhile once it is taken in
  (`participant::Follow`), where the channel's second resolving refused it before. Step 5b's text
  is aligned with it.
- **2026-11-19** — The step in which each object asks its own of the one taken in committed. Step
  5b-2 implemented, not committed yet: one function lets someone go from the session,
  `procedure::participant::let_go_from_session`, shared by `LetGo` and the sub-agents' `Ending`,
  another from one channel, `let_go_from_channel` - split at the maintainer's request -, with
  the way out as `Exit` (`Removed`, `Leaving`) in place of `GoingOut` and the channel's `Exit`. Each
  object asks its own (`SessionLoop::require_exit`, `ChannelLoop::require_exit`); the session takes
  them off its list (`unlist_participant_held`), and the procedure goes through its channels, the
  running ones at once (`ChannelLoop::let_go_held`), one that does not run in its stored document
  (`SessionLoop::let_go_where_unavailable_held`). The loops' `remove_participant_held`,
  `leave_held`, `let_go` and `let_go_with_the_session` go, and so do `changing` of both loops -
  every change of a participant is a procedure's that holds them - and with it `LocksById`. As the
  maintainer decided, `ChannelLoop::follow_agents`, whose exclusion now rests on its callers alone,
  takes the proof of admission in step 5e.
- **2026-11-19** — Step 5b-2 committed. Step 5c, its part of channels, implemented, not committed
  yet: `procedure::channel::Create`, `Delete` and `Restore` orchestrate what the building blocks
  `create_channel_held`, `delete_channel_held` and `restore_channel_held` did, the rollbacks
  becoming compensations. The session asks first by questions of its own that store nothing
  (`channel_asked_for`, `channel_to_delete`, `channel_to_restore`), and offers one step each, with
  its counterpart: storing a channel and purging it, its group kept, listing and unlisting it,
  running and stopping it (`stop_channel_held`, shared by deleting it and the compensation of
  creating it), marking it deleted and unmarking it, deleting and restoring its group,
  deleting and restoring its scope. `roll_back_channel_creation`, `roll_back_channel_deletion` and
  `abandon_channel_held` go. As the maintainer decided, a deletion whose mark the store no longer
  finds - only by another process, under the channel's lock - answers `ChannelNotFound` rather
  than `false`, so that its compensation runs.
- **2026-11-19** — Step 5c, its part of channels, committed. Its part of sessions planned in three
  steps - moving a session into a project, deleting and restoring one, creating one -, as the
  maintainer decided with a helper that undoes the steps of one session at once if they fail
  (`procedure::all_or_nothing`), for the `ProjectManager`, which deletes and restores the sessions
  of a project one by one, until the part of projects takes that over. The first implemented, not
  committed yet: `procedure::session::move_to`, shared by `Move` and the creation of a session,
  orchestrates what `SessionLoop::update_project` did. The session asks first
  (`project_to_move_to`, answering a `MoveTarget`: shared, its own, or a new own, in place of a
  flag), and offers the steps: making its new own and purging it, sharing a project with its
  participants and taking the entry away, storing where it lies. `roll_back_project_change` goes.
- **2026-11-19** — The first step of the part of sessions committed. The second implemented, not
  committed yet: `procedure::session::delete` and `restore`, shared by `Delete`, `Restore`, the
  rollback of a creation and the `ProjectManager`, orchestrate what `SessionManager::delete_held`
  and `restore_held` did. The `SessionManager` asks first (`session_to_delete`,
  `session_to_restore`), and offers the steps with their counterparts: marking the session deleted
  and unmarking it, the same of its own project, deleting and restoring the groups of its
  participants; deleting and restoring its scope, stopping it as it is deleted, announcing either.
  `roll_back_deletion` and `restore_kept` go. `procedure::all_or_nothing` undoes the steps of one
  part at once if it fails, and lets them go once it is done: the `ProjectManager` deletes and
  restores each session of a project so, as does the executor a whole procedure. What holds a
  session against the other processes is kept until the procedure is done, undone too. As the
  maintainer decided, an own project's deletion and restoring is announced with its session's, at
  the end.
