# 516 — The runtime trusts the agents linked into it

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** The runtime trusts the agents linked into it: an agent builds its own `SecurityContext`, sets the one its envelopes are checked by, reaches the policies and the managers through an open registry of services, and decides itself whether its owner is asked before a tool runs - nothing in the code says which side of the trust boundary a piece of it stands on.

## The problem

Caelumora is meant to be an operating system for agents: a kernel - `caelumora-runtime` - that
offers services, and agents that run on it and are trusted only as far as the kernel's checks let
them act. [SECURITY.md](../docs/SECURITY.md#where-permission-checks-go) says the opposite today, in
so many words: "Code linked into the program is trusted; what is kept from it is kept by the API,
not by a guarantee." Measured against the kernel view (2026-10-31), the code stands here.

**What already fits.** `ActorRuntime` is documented as "the actor's interface" and hands out
nothing about other actors (issue [69](69-scoped-actor-runtime.md)); no production code of
`caelumora-agents` or of a plugin that brings components touches `SessionLoop`, `SessionManager`
or a store directly - only tests do. Components are given narrow contexts, not the runtime:
`ComponentContext` (issue [128](128-actor-runtime-in-components.md)), `Turn`, `Situation`, the
views (issue [163](163-navigable-views.md)). The permission checks sit on the kernel's side - in
the store, the managers, at the channel's door (`ChannelLoop::admission`) -, and
[PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md) is, in effect, the table of system calls with
their checks. And the external agent is the prototype of the view: `ExternalAgentActor` is an
agent outside the process already, and "what leaves the system is decided at the door" is the
stance the kernel view takes towards every agent; what differs is the transport.

**Where the code contradicts it.**

- **An agent builds its own security context.** `AgentBody::security_context`
  (`caelumora-agents/src/actor/body.rs`) calls `AgentController::security_context(session,
  channel, limiting_roles)` and so chooses the session, the channel and the limiting roles itself.
  The `Actor` trait has a method `security_context()`: the kernel asks the actor for its rights.
  An envelope carries the context its sender's actor set, and the door checks only that the
  caller is the sender (`OriginEventSender::speaks_as`); owner, principal and roles are taken on
  trust, as issue [432](432-envelope-carries-whole-security-context.md) named it. Its variant B
  would have had the door build the context itself; it was built and taken back, because a
  context given when the request was made and one built when it is carried out are two moments
  to keep apart. Under the kernel view that decision has to be taken again.
- **`SecurityContext::system()` is public** and used in production code of a plugin
  (`caelumora-artifacts/src/manager.rs:238`, `:372`). Whoever links against the model can act as
  the system.
- **`Services` is an open registry by type.** Any component asks `context.service::<T>()` for
  whatever the runtime registered: the `AccessPolicy` and the `ConfidentialityPolicy`
  (`components/notes.rs`, `private_notes.rs`, `agent_memory.rs`, which then check themselves),
  the `SecretManager` (`caelumora-mcp`, `caelumora-a2a`), the `WorkspaceManager`, the
  `AttachmentManager`. In the kernel view that is agent code carrying out the kernel's checks,
  and a table of system calls without an end.
- **The boundary runs through `caelumora-agents`, not between crates.** On the kernel's side
  stand the admission of a tool call (`ToolExecutor::admit`, `caelumora-agents/src/tool/mod.rs`:
  offered, `CallTool`, the rules of the place, the questions, the approval - "the owner checks the
  request", [SECURITY.md](../docs/SECURITY.md#in-whose-name-an-agent-acts)), the commit of a turn,
  the storing of an agent's channel state and the actor's loop. On the agent's side stands
  everything reached through a facet: the tools, the prompt parts, the `LLMClient`, the
  `ContextProvider`, the memory, the A2A client. [AGENTS.md](../AGENTS.md) and
  [TOOLS.md](../docs/TOOLS.md) call the `ToolExecutor` "the runtime's"; the code has it in the
  agents' crate.
- **"Plugin" covers two levels of trust.** Store backends, terminal backends, the managers, views
  and REST routes of skills and artifacts are kernel modules with the kernel's trust. Tools,
  models and external agents are programs. Both are called plugin and register the same way.

## Why it matters

The design's goal is to express in the code how far an agent is trusted and what isolates it -
at compile time first, so that an agent, a tool or a model that is linked into the program can
reach the system only through an interface whose every call is checked; later, perhaps, by
running agents in processes of their own, which talk to the kernel over that interface alone and
do not link against the runtime, as programs talk to a kernel through system calls. The
compile-time cut gives most of the value: what an agent may do becomes the `pub` surface of an
API, what the kernel decides stays where it cannot be skipped. The process split is not a goal of
this issue; it is a constraint on the shape of the API - nothing that cannot cross a process:
no shared locks, no callbacks with lifetimes - so that it stays possible.

## Considerations

### The security context is handed, never built

An agent never builds its context. `ActorRuntime` hands the actor its own context; the channel
stamps the envelope at the door with the sender's context as the kernel knows it; a turn's context
is derived by the kernel from the envelope that began it (what `Answering::to` does today with
`SecurityContext::next`); `Actor::security_context()` leaves the trait. The reason 432 B was taken
back does not go away, but the answer differs from B: the kernel stamps the context when the
request enters, and keeps it in the envelope - two moments are not kept, the one at the door is
recorded. For the agent's own actions outside a request, `ActorRuntime` is asked.

`SecurityContext` becomes something agent code cannot construct. Where its constructors live is
the question, since it lies in `caelumora-model`, which every crate links: a constructor behind a
type only the kernel holds, or construction moved into the runtime. The system's context is the
kernel's alone; a plugin that needs to read as the system - the artifacts' manager - is a kernel
module and gets it as one.

### The services are closed

A component is given the facets it asks for and the few kernel services the API names - the
prompt catalogue, the secrets as a checked call (`ReadSecret`), the workspace as the `Terminal`
of its turn, the attachments and media through the turn. The policies are out of a component's
reach: notes, private notes and the memory ask the kernel - through the views, or a `may_copy` of
the turn - instead of checking themselves.

### Tools run in the agent; their admission is the kernel's

Weighed on 2026-10-31. The `ToolExecutor` is in principle part of the agent: tools are its program,
their results are what its model sees, the texts for them are its own, and it acts with the turn's
context handed to it, never with one of its own - which is so today
(`turn.situation().security_context()`). It stays in the agent.

The admission is not the agent's: it is the owner's check (SECURITY.md), and an agent that is not
trusted must not decide itself whether its owner is asked. The analogy says where the check holds:
a kernel does not keep a program from computing, it controls the resources. For tools that touch
the kernel's resources, the check at the system call is the one that holds - `ReadSecret`,
`terminal_in` for the workspace, `AttachmentManager::create_media`, the stores of notes, skills and
artifacts -, and these exist. For tools whose effect is outside the system - MCP, ComfyUI, the A2A
client - the approval is the only check, and a kernel cannot enforce it without isolating the
process and its network, as a mobile operating system does: the program asks, the system asks the
person, the system keeps the grant, and the resource checks it.

So: the admission becomes a system call of the kernel, reached through `Turn` or `ActorRuntime`,
`admit(call) -> Admission`, which asks the questions and runs the approval through the channel,
where it runs anyway (`ApproverActor`); the flow check of a result into the turn (`may_publish`)
belongs to the same decision. The agent asks for admission; it does not decide it. Whether the
resource calls later demand proof of an admission - a ticket from the kernel - is decided with
the process split, when it carries weight; today it would only pass a parameter through. What
the compile-time cut buys for tools with an outside effect is that the decision stands in one
place of the kernel and is tested there; the principle in
[ARCHITECTURE.md](../docs/ARCHITECTURE.md) is to say so.

### Kernel modules and programs

The two levels of trust that "plugin" covers are to be named and registered apart: what brings
backends, stores, managers, views and routes is a module of the kernel; what brings components
that serve an agent's facets is a program, linked against the API alone. `caelumora-agents`
holds both today and is cut along that line: the actor's loop, the commit of a turn, the storing
of state and the admission to the kernel; the facets and their built-in kinds to the programs.

### An API crate

Once the cuts above hold, the API an agent links against can be a crate of its own: the facet
traits, the narrow contexts (`ComponentContext`, `Turn`, `Situation`) and the views as traits or
opaque handles, the `Terminal`, the error types a program answers the kernel in. The runtime
implements it; the programs - `caelumora-app`, `caelumora-tui` - link the runtime and the
modules. What it costs, honestly:

- `caelumora-model` would have to be split: entities, messages and snapshots are for both sides;
  the policies, the store traits, the `Persistence` and the constructors of `SecurityContext` are
  the kernel's.
- The views are concrete types, some with lifetimes (`Situation<'a>`); as an API they become trait
  objects, with the cost of `dyn` and async.
- `AgentStateHandle` is imported sixteen times by the agents' crate: shared state under a lock,
  which crosses no process. It becomes a call to read and write one's own state.
- The workspace is the easy part: `Terminal` and `ByteStream` are streams already.

The crate is the last step, not the first: the steps before it improve the code whether or not
it is ever cut.

## The steps

Each a step of its own, reviewed and committed on its own (AGENTS.md):

1. Write the principle down - in ARCHITECTURE.md, as the extension of "the runtime decides,
   frontends map" to agents, and in SECURITY.md, in place of the sentence that trusts linked code -,
   with what it can and cannot enforce; refer to 432.
2. The security context is handed, never built: `ActorRuntime` hands the actor its context, the
   door stamps the envelope, `Actor::security_context()` goes; `SecurityContext` cannot be
   constructed by agent code, and `SecurityContext::system()` is the kernel's.
3. The services are closed to the facets and the named kernel calls; the policies leave the
   components' reach.
4. The admission of a tool call becomes a call of the kernel; the `ToolExecutor` stays in the agent
   and asks it.
5. Kernel modules and programs are told apart in how they register.
6. The API crate, with the split of the model as a decision of its own.

## Done when

- The documents state the principle and what it enforces.
- No code outside the runtime constructs a `SecurityContext` or acts as the system; the `Actor`
  trait answers no context; the door stamps what it admits.
- A component reaches no policy and no manager but through a named call of the API.
- Whether a tool call is admitted is decided in the runtime; the security suite of the agents
  states that an agent cannot run a tool its owner was not asked about where the kernel can tell.
- What brings backends, managers, views and routes registers as a module; what brings components
  as a program.

## History

- **2026-10-31** — Opened from the maintainer's thought to place Caelumora more as an operating
  system for agents: a weak trust between the kernel and the actors on it, one interface they work
  through, every interaction through the checks, and an agent never building its own
  `SecurityContext`. The code was measured against it (above). Weighed the same day: the
  `ToolExecutor` is the agent's, its admission the kernel's; the process split is a constraint on
  the API's shape, not a goal.
