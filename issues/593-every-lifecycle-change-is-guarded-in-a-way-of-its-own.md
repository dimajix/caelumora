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
3. Participants: `participant::TakeIn` (added or joining, into the session and a channel of it),
   `LetGo` (removed or leaving, from the session with every channel of it, or from one channel),
   `Follow` (agents deleted or restored), with the follower of the events; `changing` of both loops,
   `ChannelLoop::leaving`, `ChannelError::StillLeaving` and the rollbacks of taking in go. Taking
   founders in is code that creating a session or a channel and opening a thread share.
4. Channels: `channel::Create` (with its founders), `Delete`, `Restore` (letting go whom the
   session let go meanwhile), `Start` (one left out as its session started), `Reclassify`; the
   order of locks in `SessionLoop::restore_channel` goes.
5. Threads and sub-agents: `thread::Open` (with its founders) and `Close`; in
   `caelumora-subagents`, starting a sub-agent - an agent made, a thread opened, a brief delegated -
   and ending it - let go, its thread closed, the agent deleted -, in place of its `apart` and its
   rollback; `Phase` and the threads `run` keeps closing go.
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
  on it, or the reason is named why one stays as it is.

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
