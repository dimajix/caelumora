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
6. **The crate.** `caelumora-api` made of the moved modules and the traits; the libraries depend
   on it and the model; `caelumora-runtime` implements it; the dependency check in the pre-commit
   hook and CI. Files: `Cargo.toml` of the workspace and of every crate, the moved modules, the
   `use` lines. Behaviour: none.

What each step costs is in its list; what it buys is said above. Steps 2 to 4 improve the code
whether or not 5 and 6 follow.

## Step 5, measured after step 4

Read from the code on 2026-10-31, after step 4 was committed. What an application still names of
the core, outside its tests: `ActorRuntime` (the struct), `AgentController` and
`PersonalityController` (the factory of a kind of agent hands them, `AgentBody` reads `id`, `name`
and `load` of them and never writes), `AgentStateHandle` (`Situation` holds it; `current`,
`reserve_memory_id`, and `apply` at the commit of a turn), the views (`SessionView`, `ChannelView`,
`WorkspaceView`, `Views`, `Viewer`, `PrivateNotesElsewhere`), `ComponentRegistry` and
`ComponentContext` (`AgentBody` builds its container), `ActorRuntime::command_executor` (an agent's
command that shadows a session command), `channel_controller` (the two actors read the channel's
id and document), `RuntimeBuilder` and `Plugin` (every application registers through them), and
the logging spans. The plan's list above is older than steps 3 and 4; this section replaces it
for step 5. Step 5 is four steps of its own, each reviewed and committed on its own:

**5a. The readers.** Three traits, implemented by the views, which keep their full surface for
the REST server; `Situation::session_view` answers the first. Exactly what the applications call:

```rust
#[async_trait]
pub trait SessionReader: Send + Sync {
    /// The session read.
    fn id(&self) -> SessionId;
    /// The project its results go to (issue 467).
    fn project(&self) -> ProjectId;
    /// Where the reading stands: the actor's channel and its classification.
    fn place(&self) -> Option<&Place>;
    /// Channel `id`, if the reader may read it.
    async fn channel(&self, id: ChannelId) -> ViewResult<Box<dyn ChannelReader>>;
    /// The channel called `name`, if the reader may read it.
    async fn channel_named(&self, name: &str) -> ViewResult<Box<dyn ChannelReader>>;
    /// The channels the reader may read.
    async fn channels(&self) -> Vec<Box<dyn ChannelReader>>;
    /// The reader's private notes in the session's other channels.
    async fn own_private_notes(&self) -> Vec<PrivateNotesElsewhere>;
    /// How session `id` stands for the flow rule now, `None` if it is gone.
    async fn privacy_of(&self, id: SessionId) -> ViewResult<Option<SessionPrivacy>>;
    /// The extension of type `key` a plugin of the core bound to this reading (5d), if one is.
    fn extension_of(&self, key: TypeKey) -> Option<Arc<dyn Any + Send + Sync>>;
}

impl dyn SessionReader {
    /// The extension of type `T` a plugin of the core bound to this reading, if one is.
    pub fn extension<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>>;
}
```

`ChannelReader`: `id`, `name`, `classification`, `notes`, `history_page_of`, `message`,
`attachment`, `attachment_content`, `participants`, `participant`, `visible_from`,
`workspace() -> ViewResult<Option<Box<dyn WorkspaceReader>>>`. `WorkspaceReader`: `id`, `name`,
`description`, `classification`, `may_write`, `has_shell`. A trait object takes no generic
method, so `extension` is the `dyn SessionReader`'s, over `extension_of`, as
`ComponentContext::service` is over `Services`. `Views::service`, `access_policy`,
`confidentiality`, `context` and `viewer` are on no reader. Cost: the views' signatures twice,
some twenty methods, and a `Box` per reader handed out. Files: `caelumora-runtime/src/view/
{mod,session,channel,workspace}.rs`, `caelumora-agents/src/{situation,memory}.rs`, the components
and prompt parts that read, `caelumora-workspace-tools/src/workspace_prompt_provider.rs`.

**5b. `ActorRuntime` the trait.** Planned anew after 5a, in *Step 5b: the core and its
extensions* below: a slim trait, extended by one generic pattern, in three steps of its own.

**5c. Libraries register as libraries.** Planned anew after 5b, in *Step 5c: libraries* below:
what was called an application is a library of the agent side, and it registers apart from the
plugins of the core.

**5d. Skills and artifacts cut in two.** The plugin of each (store, manager, views, routes, scope
member) stays a `Plugin`; its component becomes a `Library` of the same crate, behind a feature of
its own, and reaches the manager only through a trait of the crate's API layer, which the plugin
binds to a reading (`RuntimeBuilder::extend_readers::<dyn T>(fn(&Views) -> Arc<T>)`) and the
component asks of the reader (`session.extension::<dyn T>()`):

- `SkillCatalogue`: `usable_skills`, `usable_skill_named(name)`, `create(name, scope, description,
  body, classification)` with a scope of `Session` or `Owner` - whose owner the plugin knows -,
  `reclassify`, `describe`, `rewrite`. The `SkillViews` the routes use stay as they are.
- `ArtifactPublisher`: `publish(channel, content, classification, name, description)`.

`Views::context` leaves with it (step 4's interim). Files: `caelumora-skills/src/{api,view,
component,plugin}.rs`, the same of artifacts, `Cargo.toml` of both, `caelumora-runtime/src/view/
mod.rs`.

Step 5 buys nothing a test sees: it is the shape step 6 needs, where the trait, the readers, the
`Library` and the API layers move into `caelumora-api`. Done without step 6, it costs the `Box`es
and the trait beside the struct and enforces nothing the script of step 4 does not.

## Step 5b: the core and its extensions

Decided with the maintainer on 2026-10-31, after 5a: Caelumora is a modular core - nearer the
Windows kernel than the Linux one - whose own services are few, and which is extended by
services that run on its side. An actor's interface to it, `ActorRuntime`, is slim in return,
and what lies beyond it reaches an application through one generic pattern.

### The core and what extends it

- **The core**: sessions, channels - their door and their history, the attachments of their
  messages, the approver that lives in them -, actors and their state, and persistence; with what
  they need around them: security - users, groups, the context, the policies -, the event bus,
  and projects, which sessions lie in.
- **Its extensions**: services that run on the core's side and are optional, though an actor may
  need one - the workspace and its terminal, secrets, skills, artifacts, and memory later. Each
  is a **system service**: its manager decides and keeps, asking the core's policies - extended
  by its own questions, as `SkillAccess` extends the `AccessPolicy` -, never rules of its own; its
  views serve the frontends; and what an agent's work does with it the application reaches
  through access traits the core binds to the work. A backend - a store, a storage, a terminal
  provider - is no system service: it offers an application nothing.

### The pattern of an extension

- **Two faces.** What a work may do with a service is cut in two traits: a **reader**, what it
  reads, handed out by the `Situation`, and a **writer**, what it acts by, handed out by the
  `Turn` - as the workspace's terminal is today (`Situation::workspace_terminal` reads,
  `Turn::workspace_terminal` and `workspace_shell` act). What only reads takes a `Situation`, and
  a prompt part cannot write a skill.
- **Bound by the core, per work.** A plugin binds each face to the views of a piece of work
  (`RuntimeBuilder::extend_readers`, `extend_writers`); a binding may answer none - artifacts
  without a storage of content -, so that no face carries a question of its configuration. The
  core opens the views as `session_view` does, asking `GetSession`, and the service acts in the
  work's context: no trait of a face takes a context or an identity.
- **Asked for, never kept.** A face is asked for where it is used (`Situation::extension`,
  `Turn::extension`; `Situation::offers` where only whether there is one matters) and dropped
  after: the context it was bound in is that of the call, as decision 3 wants. What it hands out
  is data, not an object of the core; a handle that carries a right beyond the call - the
  terminal - is named as one.
- **Declared.** A component kind declares the faces it needs (`ComponentKind::reads::<dyn T>()`,
  `writes::<dyn T>()`), as it declares the services it uses; the runtime refuses to be built
  where a declared face is bound by no plugin. A missing plugin is an error at start, not a `None`
  in a turn.
- **Named alike.** A reader is `…Reader`, a writer `…Writer`, as the readers of 5a are.

### What `ActorRuntime` keeps

| Group | Methods |
|---|---|
| who | `actor_id`, `session_id`, `channel_id`; `agent() -> Arc<Agent>`, `personality() -> Arc<Personality>` as they are now |
| the channel | `channel() -> Arc<Channel>`, `channel_classification`, `session_time_zone`, `announce_channel_state_changed` |
| its state | `load_channel_state`, `store_channel_state`, `transcript_message`; `agent_state() -> Arc<AgentState>`, `reserve_memory_id`, `apply_agent_state(changes, anchor)` - the memory's until it is an extension of its own |
| the attachments of messages | `load_attachments`, `create_media` |
| reading the session | `session(initiator, classification) -> Box<dyn SessionReader>` |
| decisions | `admit_tool_call`, `rule_denying`, `address_questions`, `may_publish`, `may_copy_information`, `may_reclassify` |
| texts, components | `prompts`, `build_components(config, facets)` |
| extensions | `extension_of(initiator, classification, face, key) -> Option<ErasedArc>`, and on `dyn ActorRuntime` the generic `extension::<T>(...)` over it |

Gone from it: `services`, `component_registry`, `component_context`, `session_controller`,
`channel_controller`, `command_executor` - an agent's command that shadows a session command is
left out where the core collects an agent's commands -, `workspace_terminal`, `workspace_shell`,
`max_file_bytes`, `copy_attachment` and `secrets`, which become extensions. `acting_as` stays on
`ParticipantRuntime`, behind `test-support`.

### The steps of 5b

1. **The pattern.** Two faces, bindings that may answer none, `Situation::extension`,
   `Turn::extension` and `Situation::offers`, the declaration and the check at build; `ErasedArc`
   keeps what is bound. Skills and artifacts are its first users: `SkillCatalogue` is cut into
   `SkillReader` (`usable_skills`, `usable_skill_named`) and `SkillWriter` (`owner_scope`,
   `create`, `reclassify`, `describe`, `rewrite`), `ArtifactPublisher` is a writer, bound only
   where a storage of content is configured, and `is_configured` goes. The rule goes into
   PLUGINS.md (*What a component kind outside the core may use*) and the system service into
   ARCHITECTURE.md. Files: `caelumora-runtime/src/view/reader.rs`, the runtime's
   `runtime/{actor,builder,manager}.rs` and `component/` (the declaration),
   `caelumora-agents/src/{situation,turn/mod}.rs`, the catalogue, publisher, component and plugin
   of skills and artifacts.
2. **The core's own extensions.** The workspace: `WorkspaceReader` - what `ChannelReader::workspace`
   answers today, with the terminal to read and `max_file_bytes` - and `WorkspaceWriter` - the
   terminal to change and the shell, and `copy_attachment`, which copies into the workspace -;
   secrets: `Secrets`, a reader. Both bound by the core itself, in `caelumora-runtime`; a crate of
   their own is not this issue's. `ChannelReader::workspace`, `Situation::workspace_view`,
   `workspace_terminal` and the methods of `ActorRuntime` for them go. Files:
   `caelumora-runtime/src/{runtime/actor,view/reader,workspace/*}.rs`, the tools of
   `caelumora-agents` and `caelumora-workspace-tools`, `caelumora-mcp`, `caelumora-a2a`.
3. **The terminal.** Added after step 5b.2: a workspace's terminal is cut as its extension is.
   `TerminalReader` (`list`, `stat`, `read`) is what the workspace's reader hands out, `Terminal`
   (the reader and `write`, `delete`, `mkdir`, `move_entry`) what its writer does, which asks
   `WriteWorkspace` always: reading goes through the reader, in a tool too. `shell` leaves the
   terminal for the provider, so that only the controller opens a shell, for the scope of the
   channel (`shell_in`), and the writer hands the terminal out in a hull of the core's that keeps
   the configured largest file itself, so that an application need not know it and
   `max_file_bytes` leaves the writer. Files: `caelumora-runtime/src/terminal/*.rs`, the local
   backend, `caelumora-open-terminal`, `controller/workspace.rs`, `workspace/extension.rs`, the
   REST routes of files, the tools.
4. **The trait.** `ActorRuntime` the trait of the table above, `ParticipantRuntime` the struct;
   the factory of a kind of agent is given an `Arc<dyn ActorRuntime>` alone; `agent()`,
   `personality()` and the three calls of the agent's state replace the controllers and the
   `AgentStateHandle`; `build_components` replaces the registry and the component context; the
   readers' accessors lose their `…_view` (`Situation::session`). Files:
   `caelumora-runtime/src/runtime/{actor,channel,mod}.rs`, `runtime/src/actor/kinds.rs`,
   `runtime/src/channel/participants.rs`, `caelumora-agents/src/actor/*`, `situation.rs`,
   `turn/*.rs`, the tests that build a `Turn`.
5. **Each runtime its own.** Added after step 5b.4: what only an agent asks leaves the
   `ParticipantRuntime` for `AgentRuntime`, which implements it instead of passing it on - the
   agent's state in its channel and its transcript, the attachments, its session, its extensions,
   the decisions of its work, the secrets, the texts and its components -, so that the
   `ParticipantRuntime` keeps what every participant needs: the context it acts in, the
   controllers, the channel, its events, the managers the core's actors use. `CallPermit` and
   `OwnRecord` move to the trait. The trait reads its own channel as it reads its session, through
   a reader (`ActorRuntime::channel`, which `Situation::own_channel` asks), and hands out the ids
   (`session_id`, `channel_id`) and the channel's classification directly; the channel's document
   leaves it. Files: `caelumora-runtime/src/runtime/{actor,agent}.rs`, the actors of the core, the
   views' tests that read through a runtime, `caelumora-agents/src/{actor/*,situation}.rs`.

Memory becomes a system service of its own after this issue, in an issue of its own: its
entries leave the agent's state, a reader hands out only those that may be recalled where the
work reads, and a writer stages what the turn remembers, so that the flow rule stands in the
service and the agent side never holds an entry it may not recall.

## Step 5c: libraries

Decided with the maintainer on 2026-10-31, after 5b. What runs an agent or brings components for
its facets is no application - `caelumora-comfyui` is a building block an agent uses through a
facet it defines, not a whole program -, and the word is the binaries' (`caelumora-app`). The level
is the **agent side**, a crate on it a **library**: `caelumora-agents`, which brings the kinds of
agent and the built-in components, and the crates that bring components for an agent's facets.
The core and its **plugins** keep their names; *service* stays the system service's and the
runtime's `Services`, *extension* the face a system service binds to an agent's work. Two
constructs, not one: a plugin is given the whole `RuntimeBuilder`, a library a builder that offers
what a library registers and nothing else, and from step 6 on their traits lie in different
crates - the plugin's in `caelumora-runtime`, the library's in `caelumora-api`, which a library
links alone. One trait whose implementor declared its own level would let it decide its own trust.

### The steps of 5c

1. **The word.** *Application* becomes *library* or *the agent side* wherever it names the level -
   the documents, the comments, `scripts/application-names.sh` as `scripts/library-names.sh` and
   the pre-commit hook -, and *an agent's work* where it named the work of an agent; where it names
   the binaries or the program that serves the API, it stays. No behaviour changes.
2. **The trait.** `Library` (`name`, `register(&self, library: &mut LibraryBuilder)`), beside
   `Plugin`; the `LibraryBuilder` offers `components()` - a `LibraryComponents`, which registers
   kinds as a library's -, `agent_kinds()`, `prompts()`, `config::<C>(name)` and `provide(service)`
   for a service of its own to its own kinds; `RuntimeBuilder::library(&dyn Library)` registers
   one, the binaries, `TestRuntime` and `World` list libraries apart from plugins. Beside it
   (decision 15): the runtime is not built where a kind of a library provides a facet the core
   builds in its own name (`Moderator`, `ChannelCommandProvider`) or uses a service no library
   provides but the texts (`PromptCatalogue`) - so that a library's components reach no service of
   the core either.
   Libraries: `caelumora-agents`, `-mcp`, `-a2a`, `-ollama`, `-comfyui`, `-workspace-tools`,
   `-compaction`; `caelumora-decision`, the stores and `caelumora-open-terminal` stay plugins,
   `caelumora-skills` and `caelumora-artifacts` both until 5d.
3. **What a library is given besides.** Added after 5c.2 (decision 16): a library adds its texts
   and nothing else of the catalogue (`LibraryBuilder::add_prompts`), and the configuration keeps
   the sections of the plugins and of the libraries apart, under `plugins` and `libraries`, each
   claimed only by its own; the seed files keep the plugins' sections at their top level, since
   no library claims one.

## Step 5d: the kernel

Decided with the maintainer on 2026-11-01, after 5c: what the agents run on is called the
**kernel**, in the code, the crate and the documents, in place of *core*, and of *runtime* where
that names the whole. The cut serves the picture the issue began with - an operating system for
agents -, and *kernel* says what *core* does not: that its checks are the ones nothing goes around.
A *runtime* stays what something runs with, which the kernel serves: a channel's
(`ChannelRuntime`), a participant's (`ParticipantRuntime`), an agent's actor's (`ActorRuntime`,
served by the kernel's `AgentRuntime`). The words of decision 1 become the kernel, its plugins,
trusted as the kernel, and the libraries. The plan of 5d in *Step 5, measured after step 4* is
replaced by the steps below.

### The names

| Today | Becomes |
|---|---|
| the crate `caelumora-runtime` | `caelumora-kernel` |
| `RuntimeManager` | `Kernel` |
| `RuntimeBuilder` | `KernelBuilder` |
| the module `runtime` (the kernel, its builder, the runtimes, the extensions) | `kernel` |
| `TestRuntime`, and the test helpers that answer a `RuntimeManager` (`test_runtime`, …) | `TestKernel`, `test_kernel`, … |
| `ParticipantRuntime::runtime_manager`, a variable or field that holds a `RuntimeManager` | `kernel` |
| the namespace of the texts `core` (`CORE`, `CORE_PROMPTS`, `core.framing`, a variants file `core.yaml`) | `kernel` |
| *the core*, and *the runtime* where it names the whole, in documents, comments and test names | *the kernel* |
| the feature `runtime` of `caelumora-skills` and `caelumora-artifacts` | `kernel` (5d.2) |

The files of other issues keep their words, as a resolved issue's description is never rewritten;
so do the sections of this one above, written when they were decided.

### The steps of 5d

1. **The word.** The names of the table, but the feature; `ARCHITECTURE.md` says what a kernel and
   what a runtime is. No behaviour changes. A variants file `core.yaml` becomes `kernel.yaml`, and
   a log filter names `caelumora_kernel`.
2. **Skills and artifacts cut in two.** Each crate's features: none, the entity and the wire types;
   `api`, the faces (`SkillReader` and `SkillWriter`, `ArtifactPublisher`) and their errors, which
   leave the manager - the crate's part of the API, which the kernel serves and a library calls,
   and which step 6 moves into `caelumora-api`; `kernel` (today `runtime`), the store, the
   manager, the views with their impls of the faces, the access and the plugin, without
   `caelumora-agents` or `caelumora-compaction`; `library`, the component and a `SkillsLibrary`
   or `ArtifactsLibrary` that registers its kind and its texts, on `api`; `server`, the routes. By
   default `kernel`, `library` and `server`. The programs list both libraries, the tests name the
   plugin and the library apart; `scripts/library-names.sh` checks `api.rs` and `library.rs` too,
   and the pre-commit hook builds each crate with `--no-default-features --features library`.

## Step 6: the API crate

Planned anew with the maintainer on 2026-11-01, after 5d, from what the libraries name of the
kernel outside their tests: some 55 names of 15 modules. The plan of *The API crate* above is
revised in three places (decisions 19 to 21).

### What the API holds

**`caelumora-api`** (decision 19) holds what a library is given and declares, and nothing that
decides: it depends on `caelumora-model` and on no other crate of Caelumora. The kernel depends on
it and implements its traits, and re-exports none of it, so that every type is reached one way. A
library depends on the API and the model; the kernel it links as a dev-dependency alone, for the
world of its tests (`World`, `TestKernel`), and `caelumora-agents` under `test-support` too.

- `component`: what a library declares and is given of components - `ComponentKind`, the facets
  (`ComponentFacets`), the factory, `Params` and `config_of_params`, `ComponentImpl`,
  `ComponentError`, and `ComponentContext` and `ComponentContainer` as typed faces over the graph
  the kernel builds (`ComponentGraph`: `all_of(facet, from)`, `one_of(facet, from)`), as
  `ActorRuntime::reader` is over `reader_of`; `LibraryComponents` over the kinds the kernel
  registers (`ComponentKinds`: `register(kind, factory)`, `single(facet, default_kind)`), which it
  marks as a library's. The `ComponentRegistry`, the planning, the `ref`s and the graph stay the
  kernel's, as the registry resolves the `ref`s of a configuration from the store in its owner's
  name; `ComponentContext` no longer holds that name, which the kernel keeps in its graph
  (decision 20).
- `library`: `Library`, and `LibraryBuilder` as a trait the kernel implements (decision 21):
  `components()`, `register_agent_kind(kind, facets, factory)` in place of `agent_kinds()`,
  `add_prompts(namespace, yaml)`, `section(name)` and `provide_of(key, service)`, with
  `config::<C>(name)` and `provide::<T>(service)` on `dyn LibraryBuilder`, as the pattern of
  `reader_of` and `reader::<T>` has it.
- `runtime`: `ActorRuntime`, `CallPermit`, `OwnRecord`; the four calls that answer an `anyhow`
  today - `load_channel_state`, `store_channel_state`, `transcript_message`, `apply_agent_state` -
  answer a typed error, so that the API's judgement can read it.
- `actor`: `Actor`, `Answering`; `transport`: `EventSender`, `EventReceiver`.
- `reader`: `SessionReader`, `ChannelReader`, `PrivateNotesElsewhere`, `ViewError`;
  `workspace`: `WorkspaceReader`, `WorkspaceWriter`, `WorkspaceError`; `terminal`: `Terminal`,
  `Shell`, `TerminalReader`, `ByteStream`, `WorkspacePath`, `FileEntry`, `CommandState`,
  `TerminalError`.
- `command`: `ProvidesCommands`, `CommandSet`, `CommandError`, `CommandResult`.
- `error`: `ContentError`, `AttachmentError`, `StorageGone`, and the judgement of the API's errors
  (`user_message_with`, `view_message`, `store_message`); the kernel's `user_message` adds its own
  (`SessionError`, `ProjectError`, …).
- `remote` (`ExpiringCache`, `describe_chain`, `describe_request_error`) and the spans of an agent
  (`agent_span`, `envelope_span`).

The faces of skills and artifacts stay in their crates, under their feature `api`, which depends
on `caelumora-api` instead of the kernel: in the API, the kernel would know its plugins.

### The steps of 6

1. **The seams.** Within the kernel, without a crate: every file that mixes the API with the
   kernel's own is cut, so that each part of the API stands apart and names nothing within the
   kernel - the trait of `kernel/agent.rs` from `AgentRuntime`; `command`, `terminal`,
   `transport`, `workspace/extension.rs`, `attachment`, `content` and `error` into what is given
   and what is kept; the components behind `ComponentGraph` and `ComponentKinds`; the
   `LibraryBuilder` a trait; the typed errors of `ActorRuntime`. No behaviour changes.
2. **The crate.** `caelumora-api` made of those parts, every `use` turned to it, the feature `api`
   of skills and artifacts on it. The pre-commit hook fails where `cargo tree -e normal` finds
   `caelumora-kernel` under a library; `scripts/library-names.sh` keeps the names the model still
   offers (`SecurityContext`, the policies), as the compiler refuses the rest.

The policies, the context and the store traits stay in the model (decision 22), so that a library
can still name them and the script checks that it does not; moving them is an issue of its own,
after this one.

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
8. Step 5 as four steps, 5a to 5d, in that order (recommended); or 5 and 6 together, so that
   what step 5 builds is enforced by the time it is committed.
9. The names of step 5: `ParticipantRuntime` for the struct, `ActorRuntime` staying the
   applications' word for the trait (recommended); `Application` and `ApplicationBuilder`;
   `SkillCatalogue`, `ArtifactPublisher`; `extension` and `extend_readers` for what a plugin binds
   to a reading. Answered by the steps of 5b and by decision 14: `ParticipantRuntime` and
   `ActorRuntime` as recommended, `SkillReader` (decision 12), and a `Library` with its
   `LibraryBuilder` in place of the `Application`.
10. Where the core ends (see *Step 5b*): sessions, channels with their attachments and their
    approver, actors and their state, persistence, security, the event bus and projects; the
    workspace, secrets, skills, artifacts and memory are extensions. Taken on 2026-10-31; amended
    the same day, after step 5b.2: the secrets, bound to a context, and the copying of an
    attachment for a message stay the core's.
11. Accessors of the readers without `…_view`: `Situation::session` for the session; the reader
    of the agent's own channel next to `Situation::channel`, the channel's document - recommended
    `Situation::own_channel`. Taken on 2026-10-31 as recommended; revised the same day, after
    step 5b.5: the document left the situation and the trait, so that the reader is
    `Situation::channel`, as `ActorRuntime::channel` is.
12. Whether `SkillCatalogue` is renamed `SkillReader` as it is cut, as the pattern names its faces
    (recommended). Taken on 2026-10-31 as recommended.
13. Whether the methods only an agent asks leave the `ParticipantRuntime` for `AgentRuntime`
    (*The steps of 5b*, 5), and the trait reads its channel through a reader beside its session,
    with the ids and the classification apart. Taken on 2026-10-31: both; `ActorRuntime` keeps its
    name, as other actors of applications may come; an agent's command that a command of its
    session hides is refused without naming the component that provides it.
14. What registers on the agent side, and what it is called (*Step 5c: libraries*). Taken on
    2026-10-31: a `Library`, apart from the core's `Plugin` - two constructs, as their builders and,
    from step 6, their crates differ; *service* and *extension* keep their meaning.
15. Whether the runtime refuses to be built where a component kind of a library provides a facet
    the core builds in its own name or uses a service of the core but the texts, and whether a
    library registers its kinds through a type of its own (`LibraryComponents`) rather than a
    method of the registry any plugin could call. Taken on 2026-10-31: both, as recommended.
16. Whether a library adds its texts alone rather than being handed the catalogue, and whether the
    configuration keeps the sections of the plugins and of the libraries apart; whether a library
    may declare again a single another library declared. Taken on 2026-11-01: the texts alone
    (`add_prompts`), the sections apart under `plugins` and `libraries`, and a single declared
    again is accepted, as it reaches nothing of the core.
17. The word for what the agents run on (*Step 5d: the kernel*). Taken on 2026-11-01: *kernel*,
    in the code, the crate and the documents, in place of *core*; a *runtime* is what something
    runs with.
18. The cut of 5d.2: the faces under a feature `api` of their own - the other side of the kernel
    is the API, not the agent (the maintainer, 2026-11-01) -, and the build with `library` alone
    checked by the pre-commit hook rather than by CI (recommended). Taken on 2026-11-01 as
    recommended.
19. The name of the API crate (open since decision 1): `caelumora-api`, as the other side of the
    kernel is its API (recommended), or `caelumora-kernel-api`. Taken on 2026-11-01 as recommended.
20. The cut of the components (*Step 6*): the API holds what a library declares and is given,
    typed faces over a graph and a registration the kernel implements; the registry, the
    planning and the graph stay the kernel's (recommended). Taken on 2026-11-01 as recommended.
21. The `LibraryBuilder` a trait the kernel implements, its generic helpers on `dyn
    LibraryBuilder` and its erased calls named as `reader_of` is (`provide_of`) (recommended).
    Taken on 2026-11-01 as recommended.
22. Decision 7 within step 6, or after: the policies, the context and the store traits stay in
    the model for now, checked by the script, and leave it in an issue of its own after this one
    (recommended). Taken on 2026-11-01 as recommended.
23. *Done when* names a CI the repository does not have: the pre-commit hook checks alone
    (recommended). Taken on 2026-11-01 as recommended.

## Done when

- The documents state the principle, the three levels and what the cut enforces.
- No library depends on `caelumora-kernel`, checked by the pre-commit hook (decision 23); no
  library names `SecurityContext`, a policy, a manager, a store or a controller.
- The door builds the context it decides by; `Envelope` and `Work` carry none; the `Actor` trait
  answers none.
- Whether a tool call, a result, a tool set or an agent's command is admitted is decided in the
  kernel; the agents' security suite states it.
- A library is registered as a `Library`, a plugin as a `Plugin`; `caelumora-skills` and
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

- **2026-10-31** — Decisions 8 and 9 taken as recommended: step 5 as 5a to 5d, and the names
  `ParticipantRuntime`, `Application`, `ApplicationBuilder`, `SkillCatalogue`,
  `ArtifactPublisher`, `extension` and `extend_readers`. Step 5a done on the branch: an
  application reads through readers. `SessionReader`, `ChannelReader`, `ParticipantReader` and
  `WorkspaceReader` (`caelumora_runtime::view`) are what of the views an agent's work reads, and
  the views implement them; `ActorRuntime::session_view` answers a `SessionReader`, and `Situation`
  hands out readers alone. A plugin binds a service of its own to every reading
  (`RuntimeBuilder::extend_readers`, kept in `ReaderExtensions`), made from the views the reading
  stands on and so acting in the reader's name; an application asks the reader for it
  (`SessionReader::extension`). `scripts/application-names.sh` finds the views and the plugins'
  managers in an application's code too. Differs from the plan:
  - Four readers, not three: the participants a channel lists are read through a
    `ParticipantReader`. The readers carry what the components call beyond the measured list:
    a session's and a channel's summary and briefing, a channel's language, its session and one
    participant by id.
  - The binding and the two services of the plugins come in 5a, not 5d: without them the
    components of skills and artifacts could not read past their reader. 5d is the cut into
    features alone. `SkillCatalogue` answers the `Skill` itself rather than a `SkillView`, and
    takes over `usable_skills` and `usable_skill_named` from `SkillViews`, whose one caller the
    component was; a skill's `knowledge` is the `Skill`'s. Where a skill of the owner lies it
    answers itself (`owner_scope`), in place of a scope of `Session` or `Owner`. Both services
    are implemented on `Views` itself, as the plugins' views are; what of `SkillView` only the
    component read - its id, scope, description and classification - is gone with it.
  - The core's enum of how a channel is read - from a place, or by clearance - was called
    `view::ChannelReader`; it is `ReadingBasis`, so that the name is the trait's.
  - Decision 3 is checked where the context is built, by a test of `ActorRuntime::acting_for` with
    an agent of alice's, not by the security suite of the agents any more: its test read alice's
    internal workspace through `Views::workspace`, which a reader does not offer, and an agent reads
    from its place, where its owner's clearance is not asked.
  Stored data: none.
- **2026-10-31** — Step 5a committed after the maintainer's review, who answered its questions:
  decision 3 is tested through an act besides; the `…_view` names of the readers' accessors go
  (in 5b, with the trait); an application may see a skill's access list; and whether artifacts
  can be published is asked without reading the session. Done on the branch since:
  - A plugin's service is bound to the views of a piece of work, not to a session's reader: the
    application asks its situation (`Situation::extension`, through
    `ActorRuntime::extension_of`), which opens the views as `session_view` does and asks
    `GetSession` as it does, but hands out no reader, so that `artifact_tool` asks whether a
    storage is configured without reading the session for it. `SessionReader::extension` and
    the views' `extensions` are gone; one way reaches a plugin's service. A test states the
    refusal without a storage of content (`World::configured` builds the world without one).
  - Decision 3 is checked through an act as well, in the tests of the skills
    (`an_agent_lists_the_skills_its_owners_roles_clear_it_for_as_they_are_at_each_call`): two
    calls of `list_skills` in one turn, an internal skill of alice's listed before her role is
    taken (`World::take_roles`) and not after.
  - What `TypeMap` and `ReaderExtensions` keep of a value found by its type is one type,
    `ErasedArc` (`caelumora_model::type_map`).
  - `ReadingBasis` and `channel_reading` are the crate's.
- **2026-10-31** — Step 5b planned anew with the maintainer (*Step 5b: the core and its
  extensions*): a modular core whose own services are sessions, channels, actors and
  persistence, extended by system services on its side; a slim `ActorRuntime`; one pattern for
  every extension - a reader handed out by the `Situation`, a writer by the `Turn`, bound by the
  core per work, asked for where it is used, declared by the kinds that need it. The workspace
  and secrets become the core's own extensions; memory one of its own after this issue. Decision
  10 taken.
- **2026-10-31** — Step 5b.1 done on the branch: the pattern of an extension. A plugin binds a
  reader or a writer to the views of every piece of work (`RuntimeBuilder::extend_readers`,
  `extend_writers`), a binding answering none where it is not offered; the core keeps them as its
  `Extensions` (`runtime/extension.rs`), by type and face. `ActorRuntime::reader` and `writer`
  make one for a work, asking `GetSession` as `session_view` does; a component asks its
  situation for a reader (`Situation::reader`), its turn for a writer (`Turn::writer`), and the
  situation answers whether there is a writer without handing it out (`has_writer`). A component
  kind declares what it reads and writes by (`ComponentKind::reads`, `writes`), and a runtime in
  which a declared face is bound by no plugin is not built - the declaration guards the build
  alone, since the situation and the turn know no kinds. Skills read through the `SkillReader`
  and write through the `SkillWriter`; the `ArtifactPublisher` is a writer, bound only where a
  storage of content is configured, and `is_configured` is gone. The faces of a system service
  lie in a module `extension` of its crate, in both. PLUGINS.md states the pattern (*What a
  system service offers an agent's work*), ARCHITECTURE.md the system service and where the core
  ends. Differs from the
  plan in names: `Situation::reader`, `Turn::writer` and `Situation::has_writer` for
  `Situation::extension`, `Turn::extension` and `Situation::offers`, and the typed
  `ActorRuntime::reader` and `writer` for an erased `extension_of` with a face - so that the face
  is said where one is asked for, and a reader asked of the turn is not a `None` that surprises;
  the erased form comes back in step 3, where `ActorRuntime` is a trait object. And "a writer is
  a `…Writer`" knows an exception where what it does names it better: the `ArtifactPublisher`.
  Stored data: none.
- **2026-10-31** — Step 5b.1 accepted, its questions answered as recommended: the declaration of
  an extension guards the build alone, to be looked at again in step 3, where the core builds the
  components; `ArtifactPublisher` keeps its name; the readers of the session stay the core's, the
  readers of a system service its extensions; and `artifact_tool` names the missing workspace
  before the missing storage, and answers `NotConfigured` where it is given no publisher.
- **2026-10-31** — Step 5b.2 done on the branch: the workspace is an extension of the core's.
  Its reader (`WorkspaceReader`: what is usable in the work's channel, and its files to read) is
  found as it is bound, through the views - `None` where the work may use none -, its writer
  (`WorkspaceWriter`: its files to change, its shell, the largest file it takes) asks nothing until
  it acts, and then asks what `WorkspaceController::terminal_in` and `shell_in` ask, once
  (`caelumora_runtime::workspace`, `UsableWorkspace` and `WorkspaceUse` binding them). The core
  binds both itself as the runtime is built. A binding is given the work's session as the work
  reads it (`SessionView`), as the core reached it asking `GetSession`, and may make what it binds
  asynchronously. `ChannelReader::workspace`, `Situation::workspace_view` and
  `workspace_terminal`, `Turn::workspace_terminal`, `workspace_shell` and `max_file_bytes`, and the
  methods of `ActorRuntime` for them are gone; the terminal and attachment tools, the workspace
  prompt and `artifact_tool` declare and use the faces. Differs from the plan:
  - The secrets stay the core's (`ActorRuntime::secrets`, `Situation::secrets`), not an extension:
    they are bound to a context, not to the place of a work, and an actor reads them for what it
    does outside a piece of work - the external agent's actor cancels and follows its remote
    tasks with them, also where its session cannot be read any more, and its tests run its channel
    outside a session. Bound to the views, they failed there. This corrects decision 10, for the
    maintainer to confirm.
  - `copy_attachment` stays the core's too: it copies an attachment for a message the agent will
    say, not into the workspace, as the plan had it, and the attachments of messages are the core's
    (decision 10).
  - The writer is no reader as well: it hands out the workspace's document with the terminal or
    the shell it asked for, so that acting asks the controller's questions once and not also what
    finding a usable workspace asks; and it, not the reader, says the largest file a workspace
    takes. A tool of a turn reads and changes the files through the writer, its one way to them
    (`caelumora_agents::tool::workspace_writer`); what only reads - the workspace prompt, the
    offering of tool sets - goes through the reader. The security suite records one question more
    where an agent copies an attachment into the workspace, and the artifacts' tests state what
    publishing asks: `GetSession`, as a face is handed out.
  - What a work cannot read is now answered as absence: where reading the workspace fails, the
    reader is not handed out, which is logged, and the workspace prompt says there is none, as the
    tool sets do, instead of failing the turn's context.
  - A type is bound once: a second binding of it is refused (`extend_readers`, `extend_writers`
    answer an error), so that no plugin - an application among them - replaces what the core or
    another plugin binds.
  Stored data: none.
- **2026-10-31** — Step 5b.2 accepted: the maintainer confirmed that the secrets and the copying
  of an attachment stay the core's (decision 10 amended), and that a workspace the work cannot read
  is answered as absent. The full terminal is too high a privilege to hand to what only reads: the
  terminal is cut into a reading and a changing part, as its extension is, in a step of its own
  before the trait (*The steps of 5b*, 3), which is step 4 now.
- **2026-10-31** — Step 5b.3 done on the branch: the terminal is cut as its extension is. A
  `TerminalReader` lists, looks at and reads, and is what the workspace's reader hands out; a
  `Terminal` is a reader and changes the files, and is what its writer hands out, asking
  `WriteWorkspace` always - reading goes through the reader, in a tool too (`files::read` and
  `files::change` of the terminal tool, `tool::workspace_reader`). A provider makes a terminal for
  the largest file a workspace takes (`TerminalProvider::terminal_for(id, max_file_bytes)`), which
  the terminal keeps itself, and hands a shell out for a scope (`shell_for`), which the
  workspace's controller alone asks for; the terminal has none. `max_file_bytes` left the writer,
  the `WorkspaceManager` and the `WorkspaceView`. Differs from the plan: the terminal keeps the
  largest file itself, as both backends did per write, so that no hull of the core is needed and
  one `Terminal` serves every caller, the REST routes among them; and in `caelumora-open-terminal`
  the place of a workspace in an instance (`OpenTerminal`, which its shell is made of) is apart
  from its files as a terminal (`OpenFiles`). What a tool that only reads asks is now what finding
  the usable workspace asks, then what reading it asks, as the artifacts' tests state. Stored
  data: none.
  After the review of the step: the workspace's controller cuts its terminals as the extension
  does - `reader` and `reader_in` hand out a `TerminalReader`, `terminal` and `terminal_in` a
  `Terminal` and ask `WriteWorkspace` always, without a `Grant` -, so that the REST routes that
  only read, and the workspace's reader, hold no handle that writes. The reader hands out the
  workspace as it is now with its files, as the writer does, so that what is read is labelled as
  it was checked. `tool::workspace_reader` is the one public way to the reader (it replaces
  `usable_workspace`); a provider's shell is had only where the workspace's place is there, as its
  terminal is; the terminal tool tells its tools' uses by a type (`Use`), and the reading tools'
  questions are stated by a test. Then, decided with the maintainer: the largest file is a
  storage's parameter (`max_file_bytes`, read and checked as `max_run_secs` is, with
  `default_max_file_bytes` and `check_max_file_bytes`), which its provider keeps and makes its
  terminals with (`TerminalProvider::terminal_for(id)` again); `workspaces.max_file_bytes` is gone,
  and nothing of the core - the controllers, the workspace manager - carries the value on. A
  configuration that names it fails to load, naming it, and is to name it at each storage instead.
  Differs from the plan besides: an application still reads the largest file of the storage, from
  the `TerminalReader`, where `publish_file` bounds what it offers.
- **2026-10-31** — Step 5b.4 done on the branch: `ActorRuntime` is the trait of the table above,
  the struct the runtime makes of a participant `ParticipantRuntime`. The core serves the trait from
  it as `AgentRuntime`, which holds the participant's runtime, the controllers of the agent and of
  its personality and the agent's state; the factory of a kind of agent is given an `Arc<dyn
  ActorRuntime>` alone (`AgentKinds::register`), and `AgentKinds::build` makes it, loading the
  agent's state. `agent()` and `personality()` replace the controllers in the agents' body,
  `agent_state`, `reserve_memory_id` and `apply_agent_state` the `AgentStateHandle` in the situation
  and the turn (`Situation::agent_state` a snapshot, `Turn::reserve_memory_id`), and
  `build_components` the registry and the component context. The readers' accessors lost their
  `…_view`: `Situation::session` and `own_channel` (decision 11), `ActorRuntime::session` and
  `ParticipantRuntime::session`. The controllers, the registry, the services, the component context,
  the session's commands and the agent's state are `pub(crate)` on the `ParticipantRuntime`, and
  `scripts/application-names.sh` refuses an application that names the runtime of a participant, an
  `AgentRuntime`, a `ChannelRuntime`, the `AgentStateHandle` or a controller. Differs from the plan:
  an agent's command that a command of its session would hide is refused, not left out - as the
  agents' body refused it -, by the core as it builds the actor (`AgentKinds::build`), so that the
  behaviour stays and no kind of agent can forget it; the extensions are reached through `reader_of`
  and `writer_of`, which keeps `Face` the core's; the secrets and the copying of an attachment stay
  on the trait (decision 10 amended); the trait has no `session_id`, which nothing asks, and
  `build_components` takes the defaults of the singles beside the configuration, as an external
  agent builds its own context provider by them; `Situation::new` takes the agent from the runtime,
  as `Turn::of` does. Tests make an agent's runtime with `AgentRuntime::in_test` - of the agent the
  channel resolved, else of one made up, never of a user or an approver - and `with_personality`;
  that only an agent calls tools the type says now - the core makes an `ActorRuntime` for an agent
  alone -, so that the `ToolExecutor` asks it no more, and its test went. The functions that make a
  `ParticipantRuntime` are named after it (`new_participant_runtime`, `participant_runtime_in`,
  `detached_participant_in`), and tests change a session's rules through
  `ChannelRuntime::session_controller_of`. Stored data: none.
- **2026-10-31** — Step 5b.4 accepted. The maintainer took decision 13: what only an agent asks
  moves into `AgentRuntime`, and the trait reads its own channel through a reader, as it reads its
  session, handing out the ids and the classification directly - planned as step 5b.5 (*The steps
  of 5b*, 5).
- **2026-10-31** — Step 5b.5 done on the branch: each runtime its own. `AgentRuntime` answers
  what only an agent asks itself - its state in the channel and its transcript, the attachments of
  its messages, its session and its channel to read, its extensions, the decisions of its work,
  the secrets, the texts and its components - and asks its participant's runtime for the context
  it acts in, the controllers and the channel's loop. The `ParticipantRuntime` keeps what every
  participant needs: the context, the controllers, the channel, its events and the managers the
  core's actors use; what it answered of an agent's state for a user - an error - is gone with it.
  `CallPermit` and `OwnRecord` lie beside the trait. The trait reads its own channel through a
  reader (`ActorRuntime::channel`, which `Situation::own_channel` asks), and hands out
  `session_id`, `channel_id` and `channel_classification`; the channel's document left it. The
  views' tests that read a session through a user's runtime read it from the channel's place, as
  an agent's work does. `AgentRuntime` holds the runtime of the process itself, as the channel's
  runtime does, taken once from its participant's, and asks every service of it.
  `ParticipantRuntime::actor_id`, `channel` and `subscribe_channel` are `pub(crate)`: nothing
  outside the core asks them. After the review of the step, decided with the maintainer: the
  reader of the agent's own channel is `Situation::channel`, as on the trait (decision 11
  revised), and CODING-GUIDELINES names the readers the interface of an application calls by the
  entity's name. Stored data: none.
- **2026-10-31** — Step 5c planned anew with the maintainer (*Step 5c: libraries*, decision 14):
  the agent side is made of libraries, which register apart from the plugins; the word is changed
  first, in a step of its own.
- **2026-10-31** — Step 5c.1 done on the branch: the word. Where *application* named the level, the
  documents and the comments say *library* or *the agent side* - "The core decides, the agent side
  asks" in ARCHITECTURE.md, which tells a library apart from the binaries -, and *an agent's work*
  where they named the work of an agent; `scripts/application-names.sh` is
  `scripts/library-names.sh`, and the pre-commit hook runs it so. Where *application* names the
  program that installs the logging or serves the API, it says *program*, and the media types
  stay. Where *library* meant a crate in general - the REST server, the crates that only emit log
  records, a client crate that writes a URL into its errors -, the documents and comments say
  *crate*, so that *library* names the agent side alone. The open parts of this issue follow:
  decision 9 is marked answered, step 5d names a `Library`, step 6, the plan of memory and *Done
  when* the libraries and the agent side. No behaviour changes; stored data: none.
- **2026-10-31** — Step 5c.2 done on the branch: libraries register as libraries. A `Library`
  (`caelumora_runtime::library`: `name`, `register`) registers with a `LibraryBuilder`, which offers
  its component kinds (`components()`, a `LibraryComponents`), its kinds of agent, its texts, its
  section of the configuration and a service of its own (`provide`), and nothing of the core;
  `RuntimeBuilder::library` registers one, after the plugins. `caelumora-agents`, `-mcp`, `-a2a`,
  `-ollama`, `-comfyui`, `-workspace-tools` and `-compaction` are libraries (`AgentsLibrary`, …, in
  a module `library`), and register their kinds through `LibraryComponents`; so do the components
  of skills and artifacts, whose crates register as plugins until 5d. The runtime is not built
  where a kind of a library provides a `Moderator` or a `ChannelCommandProvider`, or uses a service
  that is neither a library's nor the `PromptCatalogue` (decision 15); a plugin registers no kind
  of agent any more (`RuntimeBuilder::agent_kinds` is the core's). The programs list their
  libraries apart (`libraries()`), as `TestRuntime` and `World` do (`libraries`, beside
  `plugins`); `ScriptedAgents` and `TestAgents` are libraries, and `TestAgents` declares its model
  the default of the model's facets (`single`), since a library names no default kind.
  `scripts/library-names.sh` refuses a library that names `RuntimeBuilder`, `Plugin` or
  `ComponentRegistry`. After the review of the step: a library offers no service in place of one
  the core, a plugin or another library offers (`LibraryBuilder::provide` answers a `Result`), so
  that it cannot replace the access policy and then use it; the facets the core builds in its own
  name are those its containers ask for (`ChannelLoop::COMPONENT_FACETS`, and
  `CommandExecutor::COMPONENT_FACETS` for the commands of a session) and what the core's kinds of
  them need, on and on - the `ResponderPolicy` among them -, and a library declares no single of
  them either; the registry answers the check itself (`keeps_libraries_to_their_side`). The
  documents name the libraries as libraries throughout. Stored data: none.
- **2026-11-01** — Step 5c.3, decided with the maintainer after 5c.2: a library adds its texts and
  nothing else of the catalogue (`LibraryBuilder::add_prompts` in place of `prompts`, so that it
  replaces no variant of the core's texts), and the configuration keeps the sections of the plugins
  and of the libraries apart, under `plugins` and `libraries`, each claimed only by its own
  (`RuntimeBuilder::config`, `LibraryBuilder::config`; the builder's `Sections`). A key the
  configuration does not know is an error when it is read, at the top too. A library may still
  declare again a single another library declared (`TestAgents` its model's): the maintainer
  accepted it, since it reaches nothing of the core. Stored data: none; a configuration file moves
  `artifacts` under `plugins` and `comfyui` under `libraries`.
- **2026-11-01** — Step 5d planned anew with the maintainer (decision 17): what the agents run on
  is the kernel - the crate `caelumora-kernel`, its `Kernel` built by the `KernelBuilder`, *the
  kernel* in the documents -, and a runtime what something runs with; 5d.1 renames, 5d.2 cuts
  skills and artifacts into a plugin and a library. Decision 18 is open.
- **2026-11-01** — Step 5d.1: the kernel is named. The crate is `caelumora-kernel`, its module
  `kernel`; the `Kernel` (was `RuntimeManager`) is built by the `KernelBuilder` (was
  `RuntimeBuilder`), a test's by `TestKernel`; the texts' namespace is `kernel` (`KERNEL`,
  `KERNEL_PROMPTS`), and so is every field, variable and test helper that holds the kernel
  (`ParticipantRuntime::kernel`, `SessionManager::kernel`, `test_kernel`). *The core*, and *the
  runtime* where it named the whole, became *the kernel* in the documents, the comments, the texts
  of errors and the names of tests; ARCHITECTURE.md says that a runtime is what a channel or an
  actor runs with, given by the kernel, and that a library sees the kernel through its API alone.
  Where *the runtime* had meant the system as against the model, but the text or the rule is the
  agents', it now names them, the executor or the turn. After the review of the step: the `Kernel`
  lies in the module's `mod.rs`, as `manager.rs` is a kind's manager's; CODING-GUIDELINES.md calls
  a loop a running object, not a runtime (*Running objects and their entities*); the test
  catalogue of the model is `kernel_catalogue`. No behaviour changes. Stored data: none; a
  variants file `core.yaml` becomes `kernel.yaml`, and a log filter names `caelumora_kernel`.
- **2026-11-01** — Step 5d.2: skills and artifacts are cut in two (decision 18). Each crate's
  features: none, the entity and the wire types, whose module is `wire` now (it was `api`, which
  the feature names); `api`, the faces and their errors (`SkillError`, `ArtifactError`, which left
  the manager for `extension`); `kernel`, the manager, the views - which now serve the faces too -,
  the access and the plugin, without `caelumora-agents` or `caelumora-compaction`; `library`, the
  component and `SkillsLibrary` or `ArtifactsLibrary`, which adds its texts and registers its kind;
  `server`, the routes. The name of each (`SKILLS`, `ARTIFACTS`) lies in the crate's root, as the
  plugin and the library share it. The programs list both libraries, the tests name the plugin and
  the library apart, and a test of each crate says that the library alone keeps the kernel from
  being built. `scripts/library-names.sh` checks the faces and the libraries too; the pre-commit
  hook builds both crates with `library` alone and with `kernel` alone - the second, beyond the
  plan, so that the plugin keeps without the agents. After the review of the step: the hook builds
  them without features too, the entity and the wire types; the faces of skills need no kernel
  (`api` without `caelumora-kernel`), those of artifacts still do until step 6 (`ContentError`,
  `StorageGone`, `ByteStream`); the unused `tokio` left both crates. Stored data: none.
- **2026-11-01** — Step 6 planned anew with the maintainer, after 5d (*Step 6: the API crate*,
  decisions 19 to 23): `caelumora-api`; the components cut along the registry, which stays the
  kernel's; the `LibraryBuilder` a trait; the faces of skills and artifacts kept in their crates;
  the policies and the context left in the model for an issue of their own. 6.1 cuts the seams
  within the kernel, 6.2 makes the crate.
