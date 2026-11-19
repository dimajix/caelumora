# 601 — The session manager holds the kernel, instead of the kernel holding it

**Status:** open
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

## Done when

- The `Kernel` holds the `SessionManager` and the `ProjectManager`, built by the `KernelBuilder`,
  and no manager holds the kernel strongly in a way that makes a cycle; the option chosen and its
  reason are recorded here.

## History

- **2026-11-19** — Opened at the maintainer's request during step 3 of issue
  [593](593-every-lifecycle-change-is-guarded-in-a-way-of-its-own.md), whose procedures for
  participants need the `SessionManager` where a command runs; until this is decided, the kernel
  knows it weakly (`Kernel::sessions()`).
