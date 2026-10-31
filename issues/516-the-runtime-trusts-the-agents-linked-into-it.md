# 516 — The runtime trusts the agents linked into it

**Status:** in progress
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

## The plan

Read from the code on 2026-10-31, after the considerations above. It names the three levels, what
each crate is, what the API holds and what the kernel keeps, and answers the places that looked
hard: the security context, the tools, the workspace, the agent's state, the services of the
components, the modules that serve both sides.

### What the plan revises of the considerations above

- **The actor's loop and the commit of a turn are the program's, not the kernel's.** The loop
  (`AgentActor::run`, `caelumora-agents/src/actor/agent.rs`) is the cognitive cycle: it is what an
  agent *is*, and in a process of its own it would run there. The commit (`turn/ending.rs`) applies
  what a turn staged to the agent's own state and hands the state to the kernel to store: the
  agent's own record is the agent's, as a process's logs are the process's. What the kernel keeps
  of a conversation is the channel's history, recorded at the door. So less moves into the kernel
  than said above: the admission of acts, the flow questions and the building of contexts.
- **The external agent's actor is a program too** (`actor/external_agent.rs`): registered as a
  kind of agent like the other (`AgentKinds`), built from the same `AgentBody`, needing nothing a
  program may not have once the flow questions it asks itself (`may_publish` at lines 176, 361,
  685) are kernel calls. It is, in fact, the shape the process split will take: a kernel-side
  actor that forwards to an agent elsewhere and admits what comes back at the door.
- **The `SecurityContext` leaves the program's sight altogether**, rather than becoming a type a
  program cannot construct: Rust has no way to let one crate construct what another crate's type
  keeps private, so a type a program may name is a type a program may build. The answer is that no
  program names it (below, *The security context*).

### Three levels, three words

- **The kernel**: `caelumora-runtime`, with the policies and the context taken in from the model.
  It runs the sessions and channels, keeps the stores, decides every permission, and holds the
  actors that are the system's own: the user's (`UserActor`), the approver's (`ApproverActor`),
  and the moderators. The kernel trusts nobody outside its process but its modules.
- **A module**: a crate linked into the kernel's process with the kernel's trust, because what it
  brings *is* kernel: a backend (`caelumora-open-terminal`, `caelumora-store-json`,
  `caelumora-store-postgres`), a manager with its store, views and routes (the module layer of
  `caelumora-skills` and `caelumora-artifacts`), a component of a channel (`caelumora-decision`,
  a `ResponderPolicy`). A module implements `Plugin` and registers with the `RuntimeBuilder`, as
  today.
- **A program**: what runs an agent or serves its facets, linked against the API and the model
  alone, never against the kernel: `caelumora-agents` (both actors, the turn, the executor, the
  facets, the built-in components), and the crates that bring components for it:
  `caelumora-mcp`, `caelumora-ollama`, `caelumora-comfyui`, `caelumora-workspace-tools`,
  `caelumora-compaction`, `caelumora-a2a`, and the program layer of `caelumora-skills` and
  `caelumora-artifacts`. A program implements `Program` (new, below) and registers with the
  kernel through the API.
- **The binaries and frontends** stay what they are: `caelumora-app` and `caelumora-tui` link the
  kernel, its modules and its programs and name which are in; `caelumora-rest-server` is a
  frontend inside the kernel's process and reads through the kernel's views, as today.

The API is one crate, **`caelumora-api`**: *what a program may call of the kernel, and what the
kernel calls of a program*. It depends on `caelumora-model` and nothing else of Caelumora; the
kernel implements its traits; `caelumora-rest-api` stays the wire of REST beside it. (Alternative
name: `caelumora-kernel-api`; the shorter one is recommended, as `caelumora-rest-api` is the API
over REST.)

What enforces it: the dependency graph. A program crate lists `caelumora-api` and
`caelumora-model` and no `caelumora-runtime`; a check of the pre-commit hook and of CI runs
`cargo tree -e normal -p <program>` for every program crate and fails on `caelumora-runtime` -
the tool for the rule, as AGENTS.md asks.

### The API crate

What it holds, by where it comes from. Nothing in it decides a permission; what checks lives
behind its traits, in the kernel.

**Moved from the runtime as they are** (they name no kernel instance and both sides use them):

| From `caelumora-runtime` | To `caelumora-api` | Note |
|---|---|---|
| `component::{ComponentRegistry, ComponentKind, ComponentContext, ComponentContainer, ComponentFacets, ComponentImpl, ComponentError, ComponentResult, service_of}` with the model's `ComponentDescriptor` and `TypeKey` | `component` | the wiring is shared: the kernel builds its moderators and channel commands with it, a program its agent's components. `ComponentContext` loses `security_context` (below) |
| `transport::{EventSender, EventReceiver, EventPredicate, …}` | `transport` | already transport-agnostic: the channel's side of a process boundary |
| `actor::{Actor, ActorState}` | `actor` | the entry points the kernel calls of a program; `Actor::security_context` is gone, `Actor::commands` takes no issuer (the door filters) |
| `terminal::{Terminal, Shell, WorkspacePath, ByteStream, FileEntry, CommandState, TerminalError, TerminalResult}` | `terminal` | what a program is handed of a workspace; `TerminalProvider` and `commands.rs` stay kernel |
| `command::{Command, CommandError, CommandResult, CommandOutput, ProvidesCommands}` | `command` | what an agent's commands are made of; `CommandExecutor`, `ChannelCommandProvider`, `CommandContext` stay kernel |
| `remote` | `remote` | how a program's errors from a remote system are told |
| `content::ContentError`, `attachment::AttachmentError`, `workspace::WorkspaceError`, `view::ViewError`, `backend::StorageGone` | `error` | the errors a kernel call answers, which a program maps to a tool's (`tool/mod.rs:68-170` does it today) |

**New traits**, implemented by the kernel:

- **`ActorRuntime`** - the system calls of one actor: the trait that the kernel's struct of that
  name becomes (`runtime/actor.rs`). Every call acts for the actor the runtime was made for; no
  call takes a security context. The methods, from what the programs call today
  (`caelumora-agents`, measured: `prompts`, `channel_controller`, `channel_classification`,
  `confidentiality`, `channel_attributes`, `session_time_zone`, `load_channel_state`,
  `store_channel_state`, `announce_channel_state_changed`, `workspace_terminal`,
  `transcript_message`, `create_media`, `agent_state`, `channel`, `session_controller`,
  `command_executor`, `component_registry`, `services`, `approval_rules`, `policies`,
  `access_policy`):

  | Group | Methods | Replaces |
  |---|---|---|
  | identity | `actor_id`, `session_id`, `channel_id`; `agent() -> Arc<Agent>`, `personality() -> Arc<Personality>` as they are now | the `AgentController` and `PersonalityController` handed to the factory - the actors read them (`load`, `name`, `id`) and never write |
  | the channel | `channel() -> Arc<Channel>`, `channel_classification`, `session_time_zone`, `subscribe_channel`, `announce_channel_state_changed` | as today; `channel_attributes` goes with the agent's command check (below) |
  | the agent's state | `load_channel_state`, `store_channel_state`, `transcript_message`; `agent_state() -> Arc<AgentState>`, `apply_agent_state(changes, anchor)`, `reserve_memory_id` | `AgentStateHandle`, which stays kernel: a shared handle under a lock crosses no process; the three calls on it do |
  | reading | `session_view() -> Option<Box<dyn SessionReader>>` | `session_view(context, classification)`: the kernel knows the actor's context and the classification of its work (`Work::classification`, handed as the one argument) |
  | the workspace | `workspace_terminal(grant) -> (Workspace, Arc<dyn Terminal>)`, `workspace_shell()` | the same without the context (`WorkspaceController::terminal_in` checks in the actor's name) |
  | attachments | `load_attachments`, `create_media`, `copy_attachment` | `AttachmentManager::copy`, reached today through `context.service::<AttachmentManager>()` in `tool/attachment_tool.rs` |
  | acts | `admit_tool_call(call, event_context) -> Admission`, `admit_result(classification) -> bool`, `withdraw_approvals(turn)`, `tool_set_flows_to(destination) -> bool` | the checks of `ToolExecutor::admit`, `admitted` and `tool_sets` (below, *Tools*) |
  | flow questions | `may_copy_from(place) -> bool`, `flows_into_channel(classification) -> bool` | `ConfidentialityPolicy::may_copy_information` in `memory.rs`, `private_notes.rs`, `notes.rs`; `may_publish` in `agent.rs:463` (`lowered`), `external_agent.rs:176, 361, 685` |
  | secrets | `secrets() -> &dyn Secrets` | `SecretManager`, reached through `context.service` by `caelumora-mcp`, `caelumora-a2a` |
  | texts | `prompts() -> &Arc<PromptCatalogue>` | as today (`PromptCatalogue` stays in the model) |
  | components | `build_components(config) -> ComponentContainer` | `AgentBody::new` building its container from `component_registry()` and `services()` in a `ComponentContext` it makes with its own context (`body.rs:138-141`): the kernel builds, in the name it knows |

  `ChannelRuntime::new_actor_runtime` (`runtime/channel.rs:85`) becomes `pub(crate)`: only the
  kernel makes an actor's runtime, so a program can act for no actor but the one it was given.

- **`Secrets`** - `resolve(&SecretRef) -> SecretResult<String>`, bound to a name by the kernel:
  to the actor in `ActorRuntime::secrets()`, to the owner a graph is built for in
  `ComponentContext::secrets()` (the context "A" or "C" of today, which the kernel sets when it
  builds the container). A component that talks to a remote system resolves its token at build
  through its context and at run through its turn, as `McpToolProvider::discover` and the A2A
  client do today with a `SecurityContext` in hand. `RemoteEndpoint::bearer_token`
  (`caelumora-model/src/remote.rs:79`) takes a `&dyn Secrets` instead of the manager and a context.

- **`SessionReader`, `ChannelReader`, `WorkspaceReader`** - what a program reads of its session,
  exactly the methods the components call today and no more (measured: a session's
  `channel`, `channel_named`, `channels`, `own_private_notes`, `id`; a channel's `notes`,
  `history_page_of`, `message`, `attachment`, `attachment_content`, `participants`,
  `participant`, `visible_from`, `classification`, `name`, `id`, `workspace`; a workspace's
  `may_write`, `max_file_bytes`, `has_shell`, `classification`). The kernel's `SessionView`,
  `ChannelView` and `WorkspaceView` implement them and keep their full surface for the REST server.
  A module extends what a program may read by a trait of its own in its API layer
  (`SkillViews::usable_skills`, `usable_skill_named` today, reached through
  `views().service::<SkillManager>()`): the trait moves to the module's API layer, the module
  implements it with its manager and registers it as a service, the program declares it in
  `uses`. `Views::service`, `Views::access_policy`, `Views::confidentiality` are not on the
  readers. (The names are open: `…View` is the kernel's, `Reading` is a type of the
  confidentiality.)

- **`Program`** - what a program registers with the kernel when the binary is built: its
  component kinds (`components()`), its kinds of agent (`agent_kinds()`), its texts (`prompts()`),
  its section of the configuration (`config::<C>(name)`). The counterpart of `Plugin`, which stays
  the kernel's for modules (backends, stores, services, scope members, seed sections). The
  factory of a kind of agent (`AgentKinds::register`, `runtime/actor/kinds.rs:59`) is given one
  `Box<dyn ActorRuntime>` in place of the two controllers and the struct.

**From the model**, which the API depends on: the entities and their ids, messages, history,
transcript, context (`ContextRef`, `ModelProfile`), chain, digest, tools, questions, approvals,
`Work`, `AgentState`, `AgentChannelState`, memory, notes, plans, snapshots, `PromptCatalogue`,
`Classification`, `Place`, `Visibility`, `Grant`, `SecurityAttributes` (data on an entity),
`EventContext`, `Envelope`. These stay in `caelumora-model`.

### The kernel

What `caelumora-runtime` keeps, and takes in:

- **Taken in from the model**, because they decide, and only the kernel decides:
  `security::{SecurityContext, ActionType, Verb, Requirement, Refused, Admission, AccessPolicy,
  AccessQuestions, Enforcement, OwnershipPolicy, Policies, AllowAllPolicy}`,
  `confidentiality::{ConfidentialityPolicy, ConfidentialityQuestions, ConfidentialityRules,
  Clearance, Clearances}`, `privacy::PrivacyPolicy`, `secret::{SecretManager, SecretProvider,
  env, file}`, `user_provider`, `config::AppConfig` and what reads it. With them the security
  suite of the model. The store traits (`store::*`) stay in the model for now: they hand out no
  authority without a kernel instance, and moving 7,800 lines for hygiene alone is not this
  issue's; a program names only `StoreError` of them, which the API's `error` module covers.
  (Option weighed: a crate `caelumora-policy` for the policies alone - a crate for the sake of a
  split; rejected unless the runtime's size argues for it.)
- **Kept**: `ChannelLoop` and its door, `SessionLoop`, `SessionManager`, the controllers, the
  `Views` in full, the managers (workspaces, attachments, agents, groups, projects, secrets),
  `AgentStates`/`AgentStateHandle`, `UserActor`, `ApproverActor`, the moderators and
  `ResponderPolicy`, `CommandExecutor` and `ChannelCommandProvider` with the built-in commands,
  `TerminalProvider`, `ContentStorage`, `Backends`, `Plugin`, `RuntimeBuilder`, `AgentKinds`,
  `ScopeHolders`/`ScopeMember`, the event bus.
- **Implements** every API trait: `ActorRuntime` (the struct of today, less what moved out, plus
  the acts), `Secrets`, the readers, `Program` registration through `RuntimeBuilder`.

### The security context

The kernel owns it; no program names it. In detail:

1. **At the door.** `Envelope` loses `security` (issue 432's B, built once and taken back). The
   door builds "E" from the participant's context as the kernel holds it now -
   `Participation::context()` (`channel/participation.rs:298`) builds it already, from the
   controllers, for `require_admitted` - with the initiator the `EventContext` carries since 432.
   `OriginEventSender::speaks_as` (`transport/memory.rs:137`) keeps checking the sender.
   `ChannelLoop::security_context_of` (`channel/run.rs:396`), which builds the context of an
   envelope read back from the history, builds it the same way; `Actor::security_context` is
   removed from the trait. The decision moderator (`caelumora-decision`, `DecisionPolicy`) is
   handed the sender's context by the loop with the event, in `ResponderPolicy::choose`.
2. **A turn's context "T"** is built by the kernel at every call, from the actor the runtime is
   for and the initiator of the `EventContext` the program names (`admit_tool_call(call,
   event_context)`, the readers, the workspace): what `Answering::to` builds today
   (`runtime/actor/mod.rs:187`) and `Work` keeps for the turn. `Work` loses `security_context`;
   `Situation::security_context` goes; `Answering` keeps the event context alone. The rights are
   those of the moment of each check, not of the moment the request was given - the one moment
   432 kept, taken the other way: a role lost during a turn is lost for the turn's next act. The
   initiator lends no rights (SECURITY.md), so a program that names a false one falsifies the
   record, not its rights; the kernel can later check that it delivered what the program claims
   to answer.
3. **Components** are built in a context the kernel sets (`build_components`), and
   `ComponentContext` keeps none: what a component needs of it is `secrets()`.
4. **Facets** lose the parameter: `ExternalAgent::send`, `resume`, `cancel` (used only to resolve
   a token: they take `&dyn Secrets`, which the actor hands them from its runtime);
   `MessageProcessor::process_messages` (no implementation reads it).
5. **`SecurityContext::system()`** is the kernel's: `caelumora-artifacts/src/manager.rs:238, 372`
   is the module layer and keeps it, as a module; no program can name it.

Stored data: an agent's channel state stored while a turn was under way holds a `Work` with a
context; such states are to be deleted (as 432 noted for the `EventContext`).

### Tools

The `ToolExecutor` stays in the program, with the turn. What of `admit`
(`caelumora-agents/src/tool/mod.rs:957`) is the owner's check becomes a kernel call, and the
executor asks it:

- `ActorRuntime::admit_tool_call(call, event_context) -> Admission`: `CallTool` at the policy,
  the rules of the system, the session and the channel (`place_rule`, with the controllers the
  executor holds today and will not), the questions the call puts to people (`address_questions`,
  folded in), and the approval - the kernel sends `ApprovalRequested` in the agent's name to the
  approver, and answers `Admission::Await { approval, questions }`. The decision still reaches the
  agent as an event in its inbox (`ApprovalGranted`, `ApprovalDenied`, `ApprovalTimedOut`), as
  today: the protocol of waiting, hearing meanwhile and resuming (issues 172, 148) stays the
  program's; what changes is that only the kernel issues a request. `withdraw_approvals(turn)`
  withdraws them when a turn is given up (`agent.rs:416`).
- Whether a call names a tool it was offered (`offered`, `UnknownTool`) and whether its arguments
  fit (`providers.describe`) stay the program's: the model's to correct, no permission.
- `admit_result(classification)`: the flow of a result into the turn (`ToolExecutor::admitted`).
- `tool_set_flows_to(destination)`: whether a tool set with a destination may be offered
  (`ToolExecutor::tool_sets`).
- What the kernel can hold against a program that skips the call: nothing but the resources
  behind its other calls - secrets, the terminal, media, the state -, each checked in the actor's
  name when it is called. A ticket that those calls demand, handed out by `admit_tool_call`, is
  the step that gives the admission teeth and is not taken here.

The agent's own commands (`/compact`, `/skill`): `ShowAgentCommand` and `ExecuteAgentCommand` are
decided at the door when a `CommandRequested` naming an agent is admitted - the loop asks the
policy with the issuer's context, the agent's attributes (`ControllerManager::agent_attributes`)
and the channel's, and the agent's commands as `Actor::commands` lists them -, not by the agent
(`AgentBody::carry_out`, `AgentCommandAccess` in `caelumora-agents/src/command.rs`, which goes).
432's second decision - a command is checked with the issuer's roles as they are when it is
carried out - becomes: as they are when it is admitted, and again when it is built back from the
history (`security_context_of`). `Actor::commands` lists them all; the kernel filters what an issuer
may see where it collects them (`Participants::agent_commands`, `channel/participants.rs:437`).
`runtime.command_executor().has(name)` in `actor/commands.rs:35` - an agent's command that shadows
a session command is left out - moves to the door with the rest.

### The workspace

Easier than it looked. A program reaches the workspace by `workspace_terminal(grant)` and
`workspace_shell()` alone, which hand out the storage's `Terminal` and `Shell` for the actor's
channel after `WorkspaceController::terminal_in` checked them in its name; what it holds is a
capability for the workspace it was checked for, used until dropped, as today. `Terminal`, `Shell`
and `ByteStream` are streams and cross a process. `WorkspaceManager::max_file_bytes`, reached
through `context.service` in `tool/attachment_tool.rs` and `caelumora-workspace-tools/src/files.rs`,
is `WorkspaceReader::max_file_bytes`, which the view has. `caelumora-artifacts`' program layer
reads the file it publishes through the terminal and publishes through the module's service
(`ArtifactManager`, behind a trait of the module's API layer that takes what the kernel knows of
the caller).

### Notes, private notes, memory

`notes.rs:148`, `memory.rs:272` ask the policy `Reclassify` and `require_publish` themselves;
`memory.rs:55-133` and `private_notes.rs` ask `may_copy_information`. After the plan: a note
published into the channel is admitted at the door like a message (`require_publish` of its
classification, `Reclassify` of a note by its author - the sender -, which the door asks as it
asks `MessageReclassified`); what the agent keeps to itself - private notes, memory - it
classifies as it likes within what it saw, and asks the kernel only the flow question
`may_copy_from(place)` before it recalls. The `AccessPolicy` and `ConfidentialityPolicy` leave the
components' `uses` (`components/notes.rs:69`, `private_notes.rs:64`, `agent_memory.rs`).

### Modules that serve both sides

`caelumora-skills` and `caelumora-artifacts` are cut by features today (`runtime`: manager,
component, views, plugin; `server`: routes). They are cut again along the trust line:

- feature `module`: the entity, its store kind, the manager, the views' extension for users, the
  routes, the scope member, the `Plugin`; links the kernel;
- feature `program`: the component (tools, prompt parts, the agent's command), the `Program`;
  links the API and the module's API layer alone;
- the API layer (`api.rs` today holds the wire; it gains the trait the program calls of the
  module - `SkillCatalogue`: `usable_skills`, `usable_skill_named`, `create`; the artifacts'
  `publish`, `is_configured`): the module implements it in the name the kernel binds it to and
  registers it as a service; the program declares it in `uses`.

`caelumora-compaction` defines facets other programs serve (`KnowledgeRestorer`, served by the
skills' program layer): a program depending on a program, as today.

### What a program may hold - the whole list

An actor built by a program holds a `Box<dyn ActorRuntime>` and what it builds itself; its
components hold what their kind declared in `uses` - `Secrets`, `PromptCatalogue`, a module's
service trait - and are called with the program's own `Turn` and `Situation`, which reach the
runtime's calls and the readers. A program names, of Caelumora: `caelumora-model` and
`caelumora-api`, and the API layer of a module. It can name no policy, no manager, no store, no
controller, no loop, no context.

### What this does not enforce

- A tool whose effect is outside the system runs if the program runs it; the admission stands in
  the kernel and is tested there, and is enforced for what touches the kernel's resources.
- The agent's own record - its transcript, what the views show as "what it saw" - is written by
  the program. The channel's history, recorded at the door, is the kernel's record.
- The initiator a program names when it answers is taken as given, for the record.
- A module is trusted as the kernel is.

## The steps

Each a step of its own, reviewed and committed on its own (AGENTS.md), in this order because each
needs the one before:

1. **The principle.** ARCHITECTURE.md: the three levels and their words, as the extension of
   "the runtime decides, frontends map" to programs; what the cut enforces and what it does not.
   SECURITY.md: the sentence that trusts linked code replaced; "In whose name an agent acts" says
   that the kernel builds every context. PLUGINS.md: modules and programs. No code.
2. **The door builds "E".** `Envelope` without `security`; the door and `security_context_of`
   build from `Participation::context` and the initiator; `Actor::security_context` removed; the
   decision moderator handed the context; agent commands admitted at the door, `AgentCommandAccess`
   deleted. Files: `caelumora-model/src/actor.rs`, `caelumora-runtime/src/channel/{mod,run,
   participation,inbox}.rs`, `runtime/src/actor/{mod,user,approval}.rs`,
   `runtime/src/transport/memory.rs`, `caelumora-agents/src/actor/{body,agent,external_agent,
   commands,intake}.rs`, `caelumora-agents/src/command.rs`, `caelumora-decision/src/policy.rs`,
   `caelumora-rest-api` (`ShownEnvelope` unchanged). Tests: both security suites say the door
   decides by the kernel's context; the tests that build envelopes (some 30 `Envelope::new`).
   Revisits 432. Stored data: none.
3. **Kernel calls in place of policies in programs.** `admit_tool_call`, `admit_result`,
   `tool_set_flows_to`, `withdraw_approvals`, `may_copy_from`, `flows_into_channel`, `Secrets`,
   `copy_attachment`, `WorkspaceReader::max_file_bytes` on `ActorRuntime` (still the struct); the
   executor, the notes, the memory, the external actor, MCP and A2A call them; the `AccessPolicy`,
   `ConfidentialityPolicy`, `SecretManager`, `WorkspaceManager`, `AttachmentManager` leave every
   `uses` of a program's kind. After this step no program code names a policy or a manager.
   Files: `caelumora-runtime/src/runtime/actor.rs` (the calls), `caelumora-agents/src/tool/
   {mod,attachment_tool}.rs`, `components/{notes,private_notes,agent_memory}.rs`, `memory.rs`,
   `notes.rs`, `actor/{agent,external_agent}.rs`, `external/mod.rs`, `caelumora-mcp/src/tool.rs`,
   `caelumora-a2a/src/agent/*.rs`, `caelumora-workspace-tools/src/files.rs`,
   `caelumora-model/src/remote.rs`. Tests: the agents' security suite asserts each act through
   the kernel's call; the tests of the components with a scripted runtime.
4. **The context leaves the program.** `Work` without `security_context`; `Situation::
   security_context`, `ComponentContext::security_context` removed; every `ActorRuntime` call
   without its context parameter; the facets without theirs; `SecurityContext`, `ActionType`, the
   policies, `SecretManager`, the user providers and `AppConfig` moved from the model into the
   runtime. Files: `caelumora-model/src/{work,security/*,confidentiality,privacy,secret/*,
   user_provider/*,config}.rs` and their new places under `caelumora-runtime/src/`,
   `caelumora-agents/src/situation.rs`, `turn/*.rs`, every caller. Stored data: agent channel
   states with a turn under way. A check that no program crate names `SecurityContext` (grep in
   the pre-commit hook) until step 6 makes it impossible.
5. **The runtime becomes the program's interface.** `ActorRuntime` a trait with the method list
   above; `agent()`/`personality()` in place of the controllers; `build_components`;
   `new_actor_runtime` crate-private; `Program` beside `Plugin`, the binaries listing both;
   `caelumora-skills` and `caelumora-artifacts` cut into `module` and `program` features with the
   trait in their API layer; the readers as traits implemented by the views. Files:
   `caelumora-runtime/src/runtime/{actor,channel,builder}.rs`, `runtime/src/actor/kinds.rs`,
   `runtime/src/plugin.rs`, `runtime/src/view/{session,channel,workspace}.rs`, every program's
   `plugin.rs`, `caelumora-skills/src/{view,component,api,plugin}.rs`, the same of artifacts,
   `caelumora-app/src/main.rs`, `caelumora-tui`.
6. **The crate.** `caelumora-api` made of the moved modules and the traits; the programs depend
   on it and the model; `caelumora-runtime` implements it; the dependency check in the pre-commit
   hook and CI. Files: `Cargo.toml` of the workspace and of every crate, the moved modules, the
   `use` lines. Behaviour: none.

What each step costs is in its list; what it buys is said above. Steps 2 to 4 improve the code
whether or not 5 and 6 follow.

## Decisions for the maintainer

1. The three words - kernel, module, program - and the crate's name, `caelumora-api`. Taken on
   2026-10-31 for the words: the core, its plugins trusted as the core, and the applications
   (see History); the crate's name stays open.
2. The external agent's actor as a program (recommended), not a kernel bridge.
3. Rights at the moment of each check, built by the kernel, instead of the request's context kept
   for the turn - taking 432's decision the other way (recommended).
4. The policies, the context and the secret manager into the runtime; the store traits stay in
   the model (recommended) - or a crate of their own.
5. The names of the readers (`SessionReader`, `ChannelReader`, `WorkspaceReader`) and of the
   module's service traits (`SkillCatalogue`).
6. Whether the admission of agent commands at the door is step 2 (recommended, it is the door's)
   or a step of its own.
7. Where `SecurityContext` and the policies go, since the store traits name the context (found in
   step 4): with the store traits into the runtime, the model keeping the entities and values
   alone (recommended for step 6); or both into a crate of the core's own beside the model.

## Done when

- The documents state the principle, the three levels and what the cut enforces.
- No program crate depends on `caelumora-runtime`, checked by the pre-commit hook and CI; no
  program code names `SecurityContext`, a policy, a manager, a store or a controller.
- The door builds the context it decides by; `Envelope` and `Work` carry none; the `Actor` trait
  answers none.
- Whether a tool call, a result, a tool set or an agent's command is admitted is decided in the
  kernel; the agents' security suite states it.
- A program is registered as a `Program`, a module as a `Plugin`; `caelumora-skills` and
  `caelumora-artifacts` are cut into both.
- The API crate holds exactly what the list above names, and the kernel implements it.

## History

- **2026-10-31** — Opened from the maintainer's thought to place Caelumora more as an operating
  system for agents: a weak trust between the kernel and the actors on it, one interface they work
  through, every interaction through the checks, and an agent never building its own
  `SecurityContext`. The code was measured against it (above). Weighed the same day: the
  `ToolExecutor` is the agent's, its admission the kernel's; the process split is a constraint on
  the API's shape, not a goal.
- **2026-10-31** — The plan added, from the code: the three levels and their crates, the API
  crate and its traits, what the kernel takes in from the model, and the answers for the context,
  the tools, the workspace, the state, the components' services and the modules that serve both
  sides. It revises the considerations in two points: the actor's loop and the commit of a turn
  are the program's, and the `SecurityContext` leaves the program's sight rather than becoming
  unconstructible. The steps are rewritten in the order the dependencies ask for.
- **2026-10-31** — Step 1 begun on `issue-516-kernel-and-programs`: the principle written into
  ARCHITECTURE.md (Principles, Crates, Plugins), SECURITY.md (Operations that check themselves,
  In whose name an agent acts) and PLUGINS.md (What a plugin is, What a component kind outside the
  core may use), each naming what the code does not keep yet. The words chosen differ from the
  plan's: the documents call the kernel *the core* already, and *program* is their word for a
  binary; so the levels are **the core** with its **plugins**, trusted as the core, and the
  **applications** on it - `caelumora-agents` and the crates that bring an agent's components -,
  trusted for nothing. "Plugin" stays the word for how a crate registers until applications
  register on their own (step 5). Found while writing: an application may offer a service of its
  own to its own kinds (`caelumora-comfyui` provides its workflows), so `Program` registers such a
  service too, which the plan's list of what it registers lacks.
- **2026-10-31** — Step 2 done on the branch: the door builds "E". `Envelope` carries no
  `SecurityContext` any more (432's B, taken up again under the principle); the channel builds the
  sender's context as it knows it now - from its participants as the controllers hold them, the
  approver's answered for by the channel's owner - with the initiator the event names, and admits,
  routes and rejects in it (`ChannelLoop::security_context_of`, `admitted_in`); only who takes part
  is let in anything. `Actor::security_context` is gone; `Answering::to` continues a request from
  its `EventContext`. The moderator and the `ResponderPolicy` are handed the context with the event.
  An agent's command is admitted at the door - `ShowAgentCommand`, `ExecuteAgentCommand` with the
  agent's and the channel's attributes - and again when it is read back from the history for the
  agent; `AgentCommandAccess` is gone, the agent carries out what reached it. The commands one set
  of providers contributes are a `CommandSet`, generic; the `CommandExecutor` runs the channel's
  alone and asks for them, so the agent holds no policy for its commands. `ShownEnvelope` is gone:
  an envelope shows nothing a client may not see. Two things differ from the plan's words: a
  command read back and not admitted again is left out without a word to the issuer, who was
  answered when it entered; and the refusal of an agent's command that shadows a session command
  stays where it was (`AgentBody::command_set`, through `ActorRuntime::command_executor`) - it is a
  check of the configuration, not of a right, and leaves with the service in step 5. Stored data:
  none.
- **2026-10-31** — Step 3 done on the branch: what an application needs decided it asks of the
  core. `ActorRuntime` decides a tool call as it is admitted (`admit_tool_call`, answering a
  `CallPermit`: the policy, then the rules of the places) and again as it runs (`rule_denying`),
  what may flow where (`may_publish`), what may be copied (`may_copy_information`), whether what
  an agent keeps may be reclassified (`may_reclassify`); it binds the secrets of a work
  (`secrets`, the model's new `Secrets`), copies an attachment (`copy_attachment`) and says the
  largest file of the workspace (`max_file_bytes`). Its policies, its secret manager and its
  approval rules are the crate's. `Situation` and `Turn` offer the calls a component needs, in
  the work's name. The `ToolExecutor`, the notes, private notes and memory, both actors, the
  attachment and terminal tools, MCP and A2A ask these calls; none of their kinds declares a
  policy or a manager of the core any more. `ExternalAgent` is handed `Arc<dyn Secrets>` instead
  of a context (from step 4, since A2A needed it now), `RemoteEndpoint::bearer_token` a `&dyn
  Secrets`. Who reclassifies is the runtime's actor: the core builds the `Reclassified` itself
  from it and the channel, the application names only which of its records (`OwnRecord`).
  Differs from the plan:
  - `withdraw_approvals` is not needed: an agent withdraws its requests through the approver, the
    core's, already.
  - `admit_tool_call` folds in neither the questions nor the approval request: both stay in the
    executor, which puts the questions through `address_questions`, a call of the core already,
    and requests approval from the approver by an envelope, which the core admits.
  - The three flow questions of the plan are one, `may_publish`; `may_copy_information(from,
    into)` stands where the plan had `may_copy_from(place)` - each the name of the policy's
    question it asks.
  - The admission is two calls, as the policy is asked once and the rules twice.
  - A note's and a memory entry's reclassification is asked by the application of the core
    (`Turn::may_reclassify`), not admitted at the door: neither enters the channel as an event
    the door sees, so the door has nothing to admit.
  - The largest file of a workspace is a call of the runtime (`max_file_bytes`), not of a
    workspace's reader, which would have asked the views' questions for a number of the
    configuration.
  - The security suite was adapted, not extended: it records the same checks in the same
    contexts as before; that they are now asked by the core is not visible to it.
  Left for step 5: the artifacts' component declares the `ArtifactManager`, the views still lend
  their policies (`Views::access_policy`, `confidentiality`), and a component's context still
  offers every service. Stored data: none.
- **2026-10-31** — Step 4 done on the branch, but for the move of the types: the context leaves
  the application. `Work` carries no `SecurityContext`; the agent's runtime builds the context of
  every call its work makes of the core (`ActorRuntime::acting_for`), from the participant the
  channel resolved for it (`Acting::Participant`, as `ChannelRuntime::participant_runtime` makes
  it) and the initiator the work names - for the record, as the door does since step 2
  (`ResolvedParticipant::context_in`, now the one place both build it). Every call of
  `ActorRuntime` takes the initiator instead of a context; `Situation::security_context` is
  `Situation::initiator`; `Answering` is the event context alone; the `AgentBody` builds no
  context and its components are built in the one the core sets (`ActorRuntime::component_context`,
  `ComponentContext::security_context` the crate's); `MessageProcessor` takes none. The rights of
  a turn are those at each call, not those of the request kept for the turn - decision 3, the
  other way than 432, which the security suite now checks (an owner's roles taken away between
  two calls of one turn). Outside the tests every actor's runtime is a participant's; a test may
  make one in the actor's own name, without roles (`Acting::Itself`), or give it the context it
  needs (`acting_as`), both behind `test-support`. `scripts/application-names.sh`, run by the
  pre-commit hook, finds a `SecurityContext`, a policy or a manager of the core in an
  application's code.
  Not done: moving `SecurityContext`, the policies, the `SecretManager`, the user providers and
  `AppConfig` from the model into the runtime. The plan contradicts itself there: the store
  traits, which it leaves in the model, take a `&SecurityContext` in every method, so the context
  cannot leave the model without them. Decided with the model's split in step 6 (Decisions, 7);
  until then the check keeps applications from naming them. The components of skills and
  artifacts still hand their managers a context - the one of the views the core built for the
  turn, unnamed (`views().context()`) -, which their service traits take over in step 5. Stored
  data: none - a `Work` stored with a context reads as one without.

