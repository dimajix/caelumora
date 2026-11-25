# 601 — The session manager holds the kernel, instead of the kernel holding it

**Status:** in progress
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** Every manager of the kernel is built by the `KernelBuilder` from its parts and held by the `Kernel`, but the `SessionManager` - and the `ProjectManager` made from it - is built outside, by the server program, the terminal and the tests, and holds the whole `Kernel`, which it hands to every session, channel and actor; so nothing can reach the `SessionManager` from the kernel, and the kernel holding it would be a cycle.

## The problem

The `KernelBuilder` builds the `AgentManager`, `GroupManager`, `WorkspaceManager` and the other
managers from what each needs - the stores, the event bus, the policies (`kernel/builder.rs`) -,
and the `Kernel` holds them. They write entities; nothing runs in them, and nothing in them reaches
back to the kernel.

The `SessionManager` is built the other way round: `SessionManager::new(kernel: Arc<Kernel>)`, by
`caelumora-app`, `caelumora-tui`, the REST server's tests and some 69 places in all. It holds the
`Kernel` (`session/manager.rs`), hands it to every `SessionLoop` it starts (`session/mod.rs`), which
hands it on to every channel and actor through the `ChannelRuntime` (`kernel/channel.rs`). The
`ProjectManager` takes the kernel from it (`ProjectManager::new(sessions)`). What runs in a session
needs nearly all of the kernel: its stores, event bus, controllers, the managers of agents, groups,
workspaces and attachments, the participation, the components and kinds of agent, the services,
the prompts, the extensions.

So the kernel does not know the `SessionManager`, and what runs inside a session cannot reach it:
a command run in a channel, a follower of events, a plugin's tool call. Issue
[593](593-every-lifecycle-change-is-guarded-in-a-way-of-its-own.md) needs exactly that from step
3 on - a command lets someone go by a procedure, which is built with the `SessionManager` -, and
works around it, as decided there: the `Kernel` knows the `SessionManager` weakly, set once as it
is made (`Kernel::sessions()`). Holding it as an `Arc` would be a cycle: kernel, session manager,
running session, kernel.

## Why it matters

- The principle is that the kernel holds everything, as it holds every other manager; here a
  manager holds the kernel, and the kernel reaches it only through a weak reference.
- One manager, two of them with the `ProjectManager`, is constructed in a way of its own, at every
  place that needs it, instead of once, by the builder.
- Whoever makes a kernel and forgets the `SessionManager` has a kernel without sessions, and
  whatever reaches for them finds none.

## Considerations

The decision is open. Common to every option: the `KernelBuilder` builds the `SessionManager` and
the `ProjectManager`, and the `Kernel` holds them as it holds the other managers; the places that
build them now take them from the kernel.

- **The session manager holds the kernel weakly, the loops strongly.** Built with
  `Arc::new_cyclic`, which gives it a `Weak<Kernel>`. Running sessions and their channels still hold
  the kernel, so the cycle stands while a session runs and is gone once all are stopped: in the
  server, `procedure::session::ShutDown` stops them; a test that leaves its sessions running keeps
  its kernel - today the end of the test's runtime drops them, since only their tasks hold them. A
  rule, "who starts stops", held by review.
- **Everything below the kernel holds it weakly** - the session manager, the `SessionLoop`, the
  `ChannelRuntime`. No cycle, but every reach for the kernel becomes an upgrade with a case "the
  kernel is gone", which never happens while the process runs: more than 100 places in `session/`
  alone, its tests among them, and the channels' and actors' through the `ChannelRuntime`.
- **What runs is given what it needs, not the kernel.** The kernel is split into what sessions,
  channels and actors run with - stores, bus, controllers, the managers of kinds, components,
  services - and the kernel, which holds that and the managers of sessions and projects. The
  session manager and what runs hold the former; nothing below holds the kernel. What runs inside
  a session and has to reach the session manager - a command, a plugin - still reaches up, weakly,
  as a channel reaches its session (`kernel/channel.rs`, `session_loop: Weak<SessionLoop>`). The
  largest change, and the one that matches "the kernel holds everything" best.

## Decision

Decided by the maintainer on 2026-11-25: **the register knows the running sessions weakly, their
task owns them, and the session manager knows the kernel weakly** - a fourth option beside those
above, which corrects the first.

- **Corrected:** the `ProjectManager` is no longer made from the `SessionManager`. Since issue 593
  (steps 5d-1 and R6) the `KernelBuilder` builds it from the stores, the event bus and the
  `GroupManager` (`builder.rs`), and the `Kernel` holds it. What is left is the `SessionManager`.
- **Corrected:** the cycle runs through the register, not only through the kernel the manager
  holds: kernel, `SessionManager::sessions` (an `Arc<SessionLoop>` each), the loop's kernel. The
  first option above, a weak kernel in the manager, leaves it standing while any session is
  registered; the end of a test's runtime would no longer break it, as the manager would no longer
  be the test's to drop. Every test that leaves a session running would keep its kernel, and with
  it the `SessionHold` that holds the session in a store shared by several processes (issue 554).
- **Who owns a running session is its task.** The register holds each loop weakly; the task it
  runs in holds it strongly, so the runtime owns it - as already today, once the manager is
  dropped. The kernel holds the `SessionManager` as an `Arc`, like every other manager: no cycle
  through a session, and no rule "who starts stops" kept by review.
- **The manager knows the kernel weakly,** made with it by `Arc::new_cyclic`, as the
  `ProcedureExecutor` is (`procedure/executor.rs`): it needs it only to start a session, whose loop
  holds it (`SessionLoop::new`), and for the undo of a session's creation, which repeats procedures
  in it (`procedure::session::record_creation`). Both reach it by an upgrade that fails with
  `ProcedureError::Gone` once the kernel is gone, as the executor's does.
- **Rejected:** the session manager holding no kernel and `Procedure::carry_out` given
  `&Arc<Kernel>` instead of `&Kernel`. It would build the manager from its parts alone, as the
  others, but change the signature of every procedure - 33 of them - for the few that start a
  session. Rejected too: everything below the kernel holding it weakly (option 2), more than 100
  upgrades with a case that never happens; and splitting the kernel (option 3), whose cut would
  leave nothing outside but the `SessionManager` - the executor, which what runs needs, would still
  reach up weakly -, a second concept named only by where no cycle arises.

## Plan

Two steps, each a refactoring of its own. The first comes first: without it, the second makes the
cycle. The pattern is the `ProcedureExecutor`'s - held by the kernel, made with it, knowing it
weakly -, and for the register, a channel's handle to its loop (`kernel/channel.rs`,
`ChannelHandle`): what is owned elsewhere is reached by a `Weak`.

### Step 1: the register knows the running sessions weakly

```rust
// caelumora-kernel/src/session/manager.rs
pub struct SessionManager {
    /// The sessions that run here, each owned by the task it runs in, which holds its loop
    /// (`start`): found here as long as it runs (issue 601).
    sessions: DashMap<SessionId, Weak<SessionLoop>>,
    // the other fields unchanged
}
```

- `start` registers the loop weakly; its task holds it, as it does already - and once its `run`
  failed or panicked, until it is stopped, so that the session stays registered as running, its
  hold and handle with it, as `SessionLoop::run` says.
- `running`, `find` and `running_ids` upgrade: a session whose task ended with the runtime it ran
  in is no longer found running.
- `stop_running` upgrades what it removes, to end its actors and announce its channels stopped.

Tests: in `session/manager.rs`, a session whose task ended is not found running, nor listed among
the running ones. Documents: the doc comments of `SessionManager` and `start`.

### Step 2: the kernel holds the session manager

```rust
// caelumora-kernel/src/kernel/mod.rs
impl Kernel {
    /// The sessions of the process: the register of those that run here and their lifecycles
    /// (issue 601).
    pub fn session_manager(&self) -> &Arc<SessionManager>;

    /// The root of the views (issue 163) for `viewer`: every session it may read, running or not.
    pub fn views(&self, viewer: Viewer) -> Views;
}

// caelumora-kernel/src/session/manager.rs
impl SessionManager {
    /// The session manager of the kernel `kernel` - made with it, which holds it -, over `stores`,
    /// held against the other processes by `locks`, announcing on `events`.
    pub(crate) fn new(
        kernel: Weak<Kernel>,
        stores: &EntityStores,
        locks: Option<Arc<dyn SessionLocks>>,
        policies: Policies,
        events: Arc<EventBus>,
        groups: Arc<GroupManager>,
    ) -> Self;

    /// The kernel it starts its sessions in - for an undo that repeats procedures, and so holds
    /// it (`procedure::session::record_creation`).
    ///
    /// # Errors
    /// `ProcedureError::Gone` once it is gone, as the process ends.
    pub(crate) fn kernel(&self) -> Result<Arc<Kernel>, ProcedureError>;
}

// caelumora-kernel/src/procedure/follower.rs
impl SessionFollower {
    /// Follows the sessions `kernel` runs, from now on.
    pub fn spawn(kernel: &Arc<Kernel>) -> Self;
}
```

- `KernelBuilder::build` makes the `SessionManager` in `Arc::new_cyclic`, beside the executor.
- Removed: the field `Kernel::sessions` as an `RwLock<Weak<_>>`, `Kernel::sessions`,
  `Kernel::know_sessions`, `procedure::sessions_of` and the fallbacks of its callers for a manager
  that is gone (`procedure/channel.rs`, `participant.rs`, `session.rs`, `thread.rs`), which take
  `kernel.session_manager()`; `SessionManager::views`, which becomes `Kernel::views`;
  `AppState::session_manager` in the REST server, whose handlers read `state.kernel.views(…)`; the
  test helper `SessionManager::kernel_access_policy`, whose one caller asks the kernel.
- Removed as well, at the maintainer's request: `AppState::procedures`, a copy of what the kernel
  holds as `AppState::session_manager` is. The handlers run their procedures by
  `state.kernel.procedures()`; the program and the tests of the plugins' routes
  (`caelumora-artifacts`) no longer set it.
- `ProcedureError::Gone` names the kernel alone.
- The places that make a manager - `caelumora-app`, `caelumora-tui`, `plugin.rs`, the test helpers
  and some 65 tests - take it from the kernel. The test helpers `owned`, `owned_kernel` and
  `owned_kernel_under` answer the kernel alone, not the pair: the manager is a part of it.
- The test helpers on `SessionManager` (`session/testing.rs`) reach the kernel by
  `SessionManager::kernel`, expecting it.
- The `Compensation` stays as it is. Proposed in the discussion was that it hands the kernel to the
  steps it undoes, so that `record_creation` needs no `Arc<Kernel>`; it would change all 32 steps
  recorded and `all_or_nothing` for that one caller, which takes the kernel from the manager
  instead.

Tests: a kernel whose sessions run is let go once the runtime that runs them ends - a plain
`#[test]` that builds the kernel in a runtime of its own, starts a session, drops the runtime and
every reference, and finds the kernel's `Weak` dangling. The existing tests, with their managers
taken from the kernel.

Documents: ARCHITECTURE.md (the kernel holds the `SessionManager`; the procedures reach it so; a
running session is owned by its task, which the register only finds - what keeps the kernel free
of a cycle),
SECURITY.md (`Kernel::views` as the root of the views), REST-SERVER.md (`AppState`), CHANNELS.md and
ARCHITECTURE.md where the follower is started "beside the `SessionManager`". No stored data changes.

## Done when

- The `Kernel` holds the `SessionManager` and the `ProjectManager`, built by the `KernelBuilder`,
  and no manager holds the kernel strongly in a way that makes a cycle; the option chosen and its
  reason are recorded here.

## History

- **2026-11-19** — Opened at the maintainer's request during step 3 of issue
  [593](593-every-lifecycle-change-is-guarded-in-a-way-of-its-own.md), whose procedures for
  participants need the `SessionManager` where a command runs; until this is decided, the kernel
  knows it weakly (`Kernel::sessions()`).
- **2026-11-25** — Decided by the maintainer: the register knows the running sessions weakly,
  their task owns them, and the session manager knows the kernel weakly, made with it as the
  `ProcedureExecutor` (see *Decision*). Corrected: the `ProjectManager` is the builder's since issue
  593, and the cycle runs through the register. The plan is put before the maintainer.
- **2026-11-25** — Plan accepted by the maintainer, with `AppState::procedures` removed in step 2
  as well. Step 1 implemented: the register holds the loops weakly (`SessionManager::sessions`).
  The plan is amended where the implementation found it wrong: a session whose `run` failed or
  panicked is kept by its task until it is stopped, as `SessionLoop::run` documents that it stays
  registered as running - the plan as accepted had it found running no longer, which would have
  left its hold and handle behind and announced it stopped never. The plan names, too, that step 2
  documents who owns a running session. Test: `a_session_whose_task_ended_is_not_found_running`.
- **2026-11-25** — Step 1 accepted by the maintainer and committed. Step 2 implemented: the
  `KernelBuilder` makes the `SessionManager` with the kernel, which holds it
  (`Kernel::session_manager`), and the manager knows the kernel weakly; `Kernel::views` is the root
  of the views; `Kernel::sessions`, `know_sessions`, `sessions_of`, `SessionManager::views`,
  `AppState::session_manager` and `AppState::procedures` are gone; the follower is spawned with the
  kernel. Beside the plan: the test fixture `World` keeps its field `manager`, now the kernel's,
  which some 400 places of the tests read; the terminal's test fixture `InDirectory` loses its
  `manager`, and its `sessions_of` the parameter it no longer needed; the test helper
  `kernel_access_policy` had no caller already; `SessionManager::new` takes `&Policies`, as the
  policies travel as a pair; the follower, spawned with the kernel, holds its session manager and
  its executor, not the kernel. Test:
  `a_kernel_whose_sessions_run_is_let_go_once_their_runtime_ends`.
