# 370 — The runtime holds the environment agents run in and the agents' own logic in one crate

**Status:** done
**Area:** Code structure

## The problem

`caelumora-runtime` is "everything that runs" ([ARCHITECTURE.md](../docs/ARCHITECTURE.md)): the
sessions and channels, their actors, the controllers, the stores and the event bus - and, in the
same crate, everything an agent does once it is in a channel: its turn (`agent/turn/`), the tools
and the `ToolExecutor` (`tool/`), the prompt providers (`prompt/`), the model client (`llm/`), the
compaction (`compaction/`), the message stages (`message.rs`), the facet of an agent that runs
elsewhere (`external/`), the memory and notes logic (`agent/memory.rs`, `agent/notes.rs`), the
built-in components (`component_impl/`) and both actors of an agent (`actor/agent/`,
`actor/external_agent.rs`, with their shared `body`, `intake`, `commands` and `run_control`,
issues [355](355-two-agent-actors-share-half-their-fields.md) and
[363](363-run-control-built-twice.md)). That is 14,400 of the crate's 42,500 lines.

The two halves are meant to meet at one place: `ActorRuntime` (issue
[69](69-scoped-actor-runtime.md)) is "what an actor runs with", the `Actor` trait what a channel
asks of a participant. Measured, the environment keeps to that: the `UserActor`, the
`ApproverActor`, `channel/`, `moderator/`, `controller/`, `session/` and `workspace/` name neither
`Turn` nor a tool, a prompt provider or the model. The agent's side does not: `AgentActor` imports
from twelve modules of the runtime, and reaches past `ActorRuntime` through its `pub(crate)` doors
(`channel_loop`, `channel_handle`, `session_controller`) and those of `ViewsFactory`
(`channel_loop`), into `ChannelLoop` itself. Within one crate, `pub(crate)` cannot say "the actor
of an agent does not see the channel's inside"; whoever adds a shortcut adds it unseen.

Two other things follow from the one crate. `ComponentRegistry::built_in` registers the kinds of
`tool`, `prompt`, `llm`, `external`, `compaction` and `component_impl` as if the registry knew them,
although since issues [325](325-mcp-in-a-crate-of-its-own.md) and
[327](327-open-terminal-in-a-crate-of-its-own.md) the core is meant to know no kind. And the
channel is tested only through agents: `channel/tests/` (3,900 lines) drives every test with an
`AgentActor` or an `ExternalAgentActor` behind a scripted model or remote; there is no actor the
channel's own behaviour - routing, admission, moderation, participation, approvals - could be
tested with on its own.

## Why it matters

Every refactoring of the core since [74](74-refactoring-epic.md) has aimed at interfaces that say
what crosses them - the `ActorRuntime` of 69, the components' `ComponentContext` of
[128](128-actor-runtime-in-components.md), the views of [163](163-navigable-views.md). Between the
environment and the agent's logic there is such an interface in intent, but nothing holds it: the
compiler does not, and the tests do not, since every test of the environment goes through an agent.
A crate boundary makes the intent a rule - what the agent's logic needs of the environment is
exactly the `pub` surface of the runtime - and gives the environment tests of its own.

## Considerations

### Where to cut

Two cuts were weighed.

- **Above the actors** - `AgentActor` and `ExternalAgentActor` into crates of their own, the turn,
  the tools and the rest staying in the runtime. Rejected: the actor drives `Turn` through a dozen
  methods (`begun`, `push_message`, `count_model_call`, `try_compaction`, `tool_calls`,
  `answering`, …) that would all become `pub` - a wide, chatty interface, the opposite of a sharp
  one - and what stayed (`llm`, `compaction`, `agent/turn`, the built-in tools, the prompt
  providers) would exist for an actor that no longer lives there.
- **Below the agent's logic** - the runtime keeps the environment, a crate `caelumora-agent` takes
  everything an agent does in a channel. This is the cut that exists already (above): the
  environment does not reach into the agent's logic, and the agent's logic reaches the environment
  through a small, nameable set of doors. This is what is done.

### One crate, not two

Both kinds of agent share their body (`AgentBody`, `intake`, `commands`, `run_control`, issue 355),
and an agent's commands run in a `Turn` for agents run here and elsewhere alike
(`AgentCommandProvider`). The shared part is larger than either mind (`AgentActor` 1,100 lines,
`ExternalAgentActor` 1,000). Two crates would be three - a middle and two thin tops - with the
middle's interface as wide as the one rejected above. `caelumora-agent` holds both minds as modules;
cutting the external mind out later stays cheap once the middle is a crate.

### What stays, what moves

| Stays in `caelumora-runtime` (the environment) | Moves to `caelumora-agent` (what an agent does) |
|---|---|
| `session/`, `channel/`, `moderator/`, `group/`, `scoped.rs`, `template.rs`, `seed.rs` | `agent/turn/`, `agent/work.rs`, `agent/memory.rs`, `agent/notes.rs` |
| `actor/mod.rs` (`Actor`, `Answering`), `actor/user.rs`, `actor/approval.rs` | `actor/agent/`, `actor/external_agent.rs`, `actor/body.rs`, `actor/intake.rs`, `actor/commands.rs`, `actor/run_control.rs` |
| `controller/`, `runtime/` (`RuntimeManager`, `RuntimeBuilder`, `ActorRuntime`), `view/`, `event.rs`, `transport/`, `recorder.rs`, `logging.rs` | `tool/` with the `ToolExecutor` and the built-in tools, `prompt/`, `llm/`, `compaction/`, `message.rs`, `external/`, `component_impl/`, `remote.rs` |
| `agent/manager.rs` (the lifecycle), `agent/state.rs` (`AgentStates`, held by the `RuntimeManager`) | `command/mod.rs`: `AgentCommandProvider` and `CommandExecutor<dyn AgentCommandProvider>` |
| `component/` (the registry, generic over facets), `backend.rs`, `plugin.rs`, `workspace/`, `terminal/` | the agent's part of `testing.rs`: the scripted model, `World::turn`, personalities and agents |
| `command/mod.rs`: `ChannelCommandProvider`, `CommandExecutor` and the user commands | `channel/tests/` and the agent's tests of the security suite |

The model is unchanged: `PersonalityKind` stays the closed enum it is - the core knows that there
are two kinds of agent; what each does is what the one crate the program links brings, as the
model is "the default the core leaves open" today.

The plugins (`caelumora-skills`, `-artifacts`, `-mcp`, `-ollama`, `-a2a`) implement the facets
(`ToolProvider`, `LLMClient`, `ExternalAgent`, the prompt providers) and are given a `Turn` or an
`AgentWork`: they depend on `caelumora-agent`. The frontends do not: `caelumora-rest-server` names
`AgentError` of the lifecycle manager alone, which stays; `caelumora-app` and `caelumora-tui` name
`LLMClient` where they name the default model, which is composition.

### The contract

What `caelumora-agent` is given of the runtime, and all it is given. Each item that is `pub(crate)`
today becomes `pub`, with a doc comment that says what it is for; the exact list is what the
compiler names in step 2 below, these are the ones known:

- `Actor`, `Answering` and its constructors (`to`, `continuing`, `own`, `in_channel`, `envelope`,
  `send`, `send_all`), `reject_retries`, `retry_target`, `failure_message` - what every actor
  is and how it speaks.
- `ActorRuntime`, as the one door: its `pub(crate)` methods `channel_loop`, `channel_handle` and
  `session_controller` are replaced by what their callers need of them - the channel's
  `classification` (the agent, its commands, the external agent), whether the channel still runs
  and its controller (the `ToolExecutor`), the session's controller (the `ToolExecutor`'s guard,
  issue 13) - or made `pub` where the whole `ChannelLoop` is really needed, and said why.
- `ViewsFactory::of_actor` and `channel_loop` (the workspace of a turn, `agent/work.rs` and
  `agent/turn/mod.rs`), `ChannelHandle`.
- `AgentStateHandle`, `AgentController` and `ChannelController` as they are `pub`;
  `ChannelController::announce_resting` stays `pub(crate)`, it is the channel's.
- The `ComponentRegistry`, `ComponentContext`, `ComponentContainer`, `ComponentImpl` as they are.

### Three mechanisms the runtime does not have yet

1. **Actor kinds.** `ChannelLoop::create_actor` builds `AgentActor` and `ExternalAgentActor` by
   name. The runtime gets a registration of actor kinds on the `RuntimeBuilder`: for each
   `PersonalityKind`, a factory that builds the actor from the channel, the controller, the
   personality and the `ActorRuntime`, and the facets its container is built with
   (`COMPONENT_FACETS`, issue [337](337-components-nobody-asks-for.md)). A personality of a kind
   nobody brought is refused when the channel is built, in `ChannelError`. The `UserActor` and the
   `ApproverActor` stay the runtime's own.
2. **Errors a person may be told.** `error::user_message` downcasts every disclosable error type
   in one `if` chain, including `LLMError`, `ToolError`, `ContextError`, `ProcessingError`,
   `ExternalAgentError` and `AgentContextError`, which move with their facets. The principle for
   plugins - "the plugin answers in the core's error types" ([PLUGINS.md](../docs/PLUGINS.md)) -
   would lose what these errors say. Instead the runtime judges the types it knows and asks
   registered judges for the rest: `RuntimeBuilder::user_messages(fn(&(dyn Error + 'static)) ->
   Option<Option<String>>)`, one per crate, `caelumora-agent` bringing the one for its types.
   `From<ViewError> for ToolError` moves with `ToolError` (orphan rule).
3. **A scripted actor.** `testing.rs` gets an `Actor` that says what it is told to say and records
   what it hears, so that the channel's tests of routing, admission, moderation, participation,
   approvals and retries run without an agent. What `channel/tests/` tests of an agent's intake,
   turns and commands moves to `caelumora-agent`; what it tests of the channel is rewritten on the
   scripted actor and stays.

### Not here

- `caelumora-model` stays as it is; nothing stored changes.
- Whether `moderator/` policies, `command/` user commands or `component_impl/` kinds that read the
  channel only (`channel_history`) belong elsewhere is not asked here: they move with what gives
  them a `Turn` or stay with what does not.
- `ChannelController::announce_resting` and the rest of what the channel does to an agent's
  membership are the channel's, not the agent's.

## Plan

Two steps, the first without a behaviour change and reviewable on its own, the second mechanical;
each in a commit of its own, step 1 in several. Restructure, then move (AGENTS.md).

**Step 1 - the contract, inside the runtime.** Nothing moves; afterwards the agent's side of the
crate reaches the environment through the doors named above and nothing else.

1. `ActorRuntime` loses `channel_loop`, `channel_handle` and `session_controller` as doors for the
   agent's side; what the five callers need of them becomes methods with a name
   (`channel_classification`, …) or a documented `pub`. `ViewsFactory::channel_loop` the same.
2. Actor kinds (mechanism 1): the `RuntimeBuilder` registers the two kinds, `ChannelLoop` builds
   through them, `caelumora-app`'s seed test asks the kinds for their facets.
3. Errors a person may be told (mechanism 2): `user_message` becomes the runtime's judgement plus
   registered judges; the agent's types are judged by a function in `agent/` registered by the
   builder.
4. The registry's built-in kinds: `ComponentRegistry::built_in` registers the runtime's own kinds
   (`moderator`, the user `command`s); the agent's kinds are registered by one function
   `agent::register_all`, called where the program and the tests build the registry.
5. The scripted actor (mechanism 3), and the channel's tests sorted: those of the channel on the
   scripted actor, those of the agent marked for the move.
6. Visibility: every `pub(crate)` the agent's side still uses becomes `pub` with its doc comment -
   the compiler lists them when the modules are made a `pub mod agent_logic` with `pub(crate)`
   cut off, or by the move itself in step 2. Review the list: each entry is a door.

**Step 2 - the move.**

7. The crate `caelumora-agent` is created; the modules of the right-hand column move by `git mv`,
   their paths changing from `caelumora_runtime::…` to `caelumora_agent::…`; the plugins, the app
   and the TUI follow. `testing.rs` is split: the environment's world in the runtime (feature
   `test-support`), the agent's on top of it in `caelumora-agent` (its own `test-support`).
8. The tests: `channel/tests/` of the agent and the agent's tests of `security_suite.rs` move to
   `caelumora-agent/tests/`; the suite's world stays the runtime's, extended by the agent crate's.
9. The documents: ARCHITECTURE.md (the crates, the plugins, "agents are made of components"),
   AGENT-TURN.md, TOOLS.md, COMPONENTS.md, PLUGINS.md, SECURITY.md (where the suite lies),
   CODING-GUIDELINES.md and AGENTS.md (the tables that name the runtime) say where things are now;
   AGENTS.md's rule about the security suite names both suites.

## Done when

- `caelumora-runtime` holds the environment alone, as the left-hand column, and can be described
  in one sentence: where sessions, channels and their actors run. `caelumora-agent` holds what an
  agent does in a channel, both kinds, and depends on the runtime; the plugins depend on it.
- The agent crate reaches the runtime through `pub` items each of which has a doc comment saying
  what it is for; `ActorRuntime` has no `pub(crate)` door the agent's side used.
- The runtime's channel tests run on the scripted actor; the agent's tests run in the agent crate;
  the security suite covers what it covered.
- `PersonalityKind` and the stored data are unchanged; clippy and the tests pass; the documents
  name the crates.

## History

- **2026-10-15** — Opened at the maintainer's request, after weighing where to cut: the first
  proposal, the two actors into two crates with the turn and the tools staying, was measured and
  rejected for the width of the interface it would open (above); the cut below the agent's logic
  was measured and found to exist already, with the doors named under *The contract*.
- **2026-10-15** — Step 1, point 1, in the working tree of the branch: `ActorRuntime` offers
  `channel_classification`, `channel_attributes` and `address_questions`, `ViewsFactory` offers
  `workspace_terminal` and `workspace_shell`, `session_controller` is `pub`; the `ToolExecutor`
  holds the agent's `ActorRuntime` instead of a `ChannelHandle`, and nothing on the agent's side
  reaches `ChannelLoop`, `ChannelHandle` or `SessionLoop` any more. This contradicts
  [144](144-runtime-and-entity-accessors.md), which removed `channel_classification` and
  `channel_attributes` from `ActorRuntime` on purpose ("the entity above is reached through the
  parent", CODING-GUIDELINES.md), and which the plan above overlooked. The alternative within
  144's rule - `ActorRuntime::channel_loop` made `pub` - would hand the agent's crate the
  frontends' API of the channel, `ChannelLoop::user_actor` among it, which acts in a user's name
  (issue 30). Awaiting the maintainer's decision whether the crate boundary is an exception to
  144's rule.
- **2026-10-15** — Decided by the maintainer: the `ActorRuntime` is the central interface of an
  actor, and what an actor needs that it lacks goes into it - the crate boundary is an exception
  to 144's rule, written down in CODING-GUIDELINES.md and CHANNELS.md. `workspace_terminal` and
  `workspace_shell` moved onto `ActorRuntime` too; `ViewsFactory` passes them on to the turns,
  which are not given the `ActorRuntime` (issue 128). Point 1 of step 1 is done.
- **2026-10-15** — Raised by the maintainer: point 1 had put `workspace_terminal` and
  `workspace_shell` on `ViewsFactory`, though views only read
  ([296](296-workspace-operations-on-the-view.md)). The cause was older: a `Turn` held a
  `ViewsFactory` as its one link to its actor, and so reached its workspace through
  `views.channel_loop()`. Decided: `Turn` and `AgentWork` hold the `ActorRuntime` - private, so
  components still see only the turn's methods ([128](128-actor-runtime-in-components.md)) - and
  derive the views from it. `ViewsFactory` lost the workspace methods and `channel_attributes`;
  in tests, `actor_runtime_in` and `detached_actor_runtime()` replace `views_in` and
  `ViewsFactory::detached`.
- **2026-10-15** — `ViewsFactory` folded into `ActorRuntime`, as proposed and accepted: it had
  one method left and one caller. `ActorRuntime::session_view(context, classification)` is the
  agent's own session, asked through the edge the views have (`Views::session`) instead of a
  second copy of its question. It reads from the actor's own channel; the channel the caller
  named before is gone as a parameter, since a public one would let a caller read as from a
  channel that is not its own. A fixture of `agent_memory` had relied on it - turns in channels
  of their own, read through the session's main channel -, and now runs those channels
  (`session::testing::running_channel`).
- **2026-10-15** — Step 1, point 2 done: actor kinds. `AgentKinds` (`actor/kinds.rs`) holds, by
  `PersonalityKind`, what builds an agent's actor and the facets its components are asked for;
  the `RuntimeBuilder` starts with the runtime's two (`AgentKinds::built_in`), the
  `RuntimeManager` keeps them, and the
  `ChannelLoop` takes them once and builds every agent's actor through them. `ResolvedKind` has
  one `Agent` variant instead of two, its personality boxed (clippy's `large_enum_variant`, which
  the second variant had hidden). A kind nobody registered is refused as
  `ChannelError::UnknownAgentKind` (REST `400 invalid_participant`). `PersonalityKind` derives
  `Hash`, a key of the registry, which changes nothing stored. The app's seed test asks the kinds
  for the facets. Registering a kind stays crate-internal for now, and so does `Actor`: see the
  next entry.
- **2026-10-15** — Found while doing point 2, not foreseen by the plan: the crate that implements
  an actor needs `Actor` public, and SECURITY.md keeps it crate-internal on purpose - its control
  methods (`cancel`, `stop`, `wakeup`, `retry_turn`, `interrupt_tool_call`) check nothing, and a
  frontend that reaches its `UserActor` (`ChannelLoop::user_actor`) must not be able to call them.
  Public, the trait would let any crate do so. Awaiting the maintainer's decision how the
  guarantee is kept across the crate boundary.
- **2026-10-15** — Decided by the maintainer: `Actor` is public. There is no boundary within the
  program the crate-internal trait could have kept: `SecurityContext::system`, the stores and the
  `RuntimeManager` are public, and the REST server and the plugins use them - code linked into
  the program is trusted. What the trait's visibility kept was frontends from calling the control
  methods by mistake; SECURITY.md now says that this is a rule for linked code.
  `AgentKinds::register` and `RuntimeBuilder::agent_kinds` are public with it; `Moderator` and
  `ChannelLoop::actor` stay crate-internal, so no agent's actor is handed out.
- **2026-10-15** — Decided by the maintainer: the agents' crate is a plugin - an essential one;
  without it the program is a chat between users. It follows the pattern a crate that brings
  registrations to the `RuntimeBuilder` already has, and `AgentKinds` with it the pattern of
  `Backends`: empty to begin with, filled by a plugin. So `AgentKinds::built_in` is gone, and
  `AgentsPlugin` (`agent_plugin.rs`, in the runtime until step 2) registers both kinds;
  `caelumora-app` and `caelumora-tui` list it first, `testing::World` registers it before the
  plugins it is given, and `RuntimeManager::new` - the core without plugins, which the tests build -
  registers it alone, until step 2, when the runtime can no longer name it. What the plan says of
  the agents' registrations changes with it: in point 4, the agent's component kinds are
  registered by `AgentsPlugin` (`components()`) rather than by an `agent::register_all` the
  programs call; in point 3, the judges of the agent's errors are registered by it too; in step 2,
  the crate's `plugin.rs` is `AgentsPlugin`, and the runtime's tests that need agents build their
  runtime with it.
- **2026-10-15** — Step 1, point 3 done, with another mechanism than the one planned: no judges
  registered with the runtime. `user_message` is a function without state, called where no
  runtime is at hand (the REST server's `ApiError::internal`, the TUI, user commands), and a
  registry of judgements is what [336](336-errors-of-a-plugin-shown-as-a-failure.md) rejected as "a
  registry of downcasts beside the chain". Instead, as 336 has it, whoever defines an error type
  judges it: the runtime its own in `error::user_message`, which now walks the chain asking
  another judgement first if it is given one (`user_message_with`), with `view_message` and
  `store_message` public for the errors that hold those; the agents' logic the error types of its
  facets - `LLMError`, `ExternalAgentError`, `ToolError`, `ContextError`, `ProcessingError` - in
  `agent_error::user_message` (in the runtime until step 2, then the crate's `error.rs`), which
  `failure_message` uses and which moved there from `actor/mod.rs`. Measured: those five types
  reach no frontend; one that did would be shown as a failure pointing to the log, as 336 says of
  a plugin's type. The agents' plugin registers no judgement, and the plan's point 3 of the
  agents' registrations falls away. CODING-GUIDELINES.md and PLUGINS.md say so.
- **2026-10-15** — Step 1, point 4 done: `ComponentRegistry::built_in` registers the runtime's own
  kinds alone - `command` and `moderator` -; the kinds of an agent's components (`component_impl`,
  `prompt`, `tool`, `llm`, `external`, `compaction`) are registered by `AgentsPlugin`, through
  `agent_plugin::register_components`, which `testing::test_registry` calls too for the tests
  that build components without a runtime. That made one thing visible: a test's registry names
  its model before the plugin declares `LLMClient` a single, and the declaration replaced the
  program's choice. The registry keeps the default kinds the program named
  (`set_default_kind`) apart from the declarations now, so that declaring a single again keeps
  the program's choice - as the app and the TUI already do in the right order, after their
  plugins.
- **2026-10-15** — Step 1, point 5 done: a scripted agent. `testing::ScriptedAgent`
  (`testing/scripted_agent.rs`; `testing.rs` became `testing/mod.rs`) is an actor of a kind of
  agent with nothing of an agent's logic: asked to respond, it begins a turn and answers at once
  or hangs until it is cancelled, asked again or stopped (`Script`), tells where it stands, and
  keeps what it heard and whether it was woken. `ScriptedAgents(script)` is a plugin that
  registers it for both kinds of personality - the registry of kinds made it a plugin, not a
  fixture. `RuntimeManager::new` takes the plugins it builds with, so it no longer registers the
  agents' plugin by itself; the runtime's test helpers (`session::testing::test_runtime`,
  `owned_runtime` and those of the views, actors and workspaces) name `ScriptedAgents`, the
  fixtures of the agents' tests `AgentsPlugin`. Measured by letting the agents' plugin register
  nothing: of the runtime's tests, 13 still needed a real agent. Five test what the channel or a
  user command does - a participant added again, `/nudge` - and run on scripted agents now
  (`channel::tests::scripted_channel`, `stuck_in_a_turn`); `a_member_s_answer_under_way…` of the
  views hangs a scripted agent instead of giving a real one a silent model. The others test an
  agent: the six commands an agent carries out moved to `channel/tests/agent_commands.rs`, and the
  two tests of an agent's facets - which a channel and an agent take, and that the configuration
  replaces the context provider - to the tests of `agent_plugin.rs`. Since then, the runtime's
  tests that fail without the agents' plugin are those of the modules that move alone. The
  security suite, which builds its world with the agents' plugin, is sorted in step 2 (point 8).
- **2026-10-15** — Step 1, point 6, first part: what the runtime still took from the agents'
  logic. Measured: three places of its code, and fixtures of its tests. The fixtures - the shared
  ones of `channel/tests/mod.rs`, `testing/mod.rs` (`World::turn`, the model not to be asked, the
  agents' plugin in the world), `workspace/testing.rs` (`WithWorkspace::turn`) and a test of
  `session/channels.rs` that names the echo tool - are split in step 2, with the tests (points 7
  and 8). The code:
  - `command/mod.rs` held `AgentCommandProvider`, which carries a command out in a `Turn`, and
    the executor's inherent `impl CommandExecutor<dyn AgentCommandProvider>`, which no other
    crate could write. The executor stays generic in the runtime, its two steps public
    (`visible_by`, `admit`, and the `CommandEntry` it admits); the facet, `AdmittedCommand` and
    what is asked of an agent's commands moved to `agent_command.rs`, the latter as the extension
    trait `AgentCommandAccess` - as a plugin adds its questions to `AccessQuestions`.
  - `view/mod.rs` held `From<ViewError> for ToolError`; it is the tool module's now.
  - `remote.rs`, put among what moves in the table above, stays: the component container of the
    runtime describes errors with it (`describe_chain`), and the plugins that talk HTTP use it
    whatever they serve.
- **2026-10-15** — Step 1, point 6, second part: what the agents' logic takes from the runtime.
  Measured by the compiler, not guessed: in a throwaway copy of the repository, the modules that
  move were cut into a crate `caelumora-agent` by a script, and every item it could not reach
  was made public there and noted, until it built. One more inherent `impl` on a type of the
  runtime came up first - `ComponentImpl<dyn PromptProvider>::prompt_for` -, now a function of
  the prompt module, as `apply_stage_processors` is of the message stages. Then the doors were:
  - `Answering` - its two fields and `to`, `continuing`, `own`, `send`, `send_all` -, public,
    so that the actors of a kind of agent build their envelopes where every envelope is built
    (issue 352);
  - `Views::privacy_of`, public, which an agent's memory asks;
  - `reject_retries` and `retry_target` of `actor/mod.rs`, which the runtime itself never used:
    not opened but moved to the agents' side, `AgentBody::reject_retries` - every caller passed
    the body's id, name and context - and `body::retry_target`.
  A fresh copy with these changes builds the agents' crate without a further door, and the
  runtime without anything left unused. Step 1 is done; what remains for step 2 is the move
  itself - the script is the start of it - and the tests and fixtures named in the entry above.
- **2026-10-15** — Step 2, first part: the tests and their fixtures are cut along the same line,
  before anything moves. Measured as for the code: in a throwaway copy the tests of the agents'
  logic were cut into the new crate with it, until they built - and ran: 258 there, 230 left in
  the runtime, the 488 of before.
  - The world of the security suite (`testing::World`) runs no agent; `agent_testing` (in the
    runtime until the move, behind `test-support`) is the world with the agents' plugin and the
    models of a test (`world`, `world_with`, …, `with_agent_kinds`, its own `test_registry`), with
    `turn` (once `World::turn`), `asked_for_a_call` (once a method of `World`), `workspace_turn`
    (once `WithWorkspace::turn`), the models `Once` and the one not to be asked, and `calling`.
    `testing::test_registry` holds the runtime's kinds alone. The 47 tests of the suite that need
    real agents, and the tests of `caelumora-skills` and `caelumora-artifacts`, build the world of
    the agents.
  - The fixtures of `channel/tests/mod.rs` the agents' tests share with the runtime's - `Stuck`,
    `start`, `settled`, `answered`, `alice_of`, `Policy`, `command_error`, alice and her agent -
    are `channel::testing`, public behind `test-support`; the agents' own, with their four test
    files, are `channel/tests/agents/`, which moves as a whole. What those tests read of a channel
    beyond its API - an actor, the whole history, the stored state of an agent, the runtime of a
    restart - is a `test-support` block of `channel::testing` (`actor_of`, `whole_history`,
    `stored_agent_channel_state`, …), so that the channel's API is not widened.
  - `session::testing`, `workspace::testing` and `component::testing` are public behind
    `test-support`, and with them `SessionManager::create`, `ChannelRuntime::outside_a_session`
    and `restarted`, `ChannelLoop::observe`, `transport::Discard` and the test constructor
    `AgentContextView::of_state`.
  - Made public for good: `ChannelLoop::COMPONENT_FACETS` and `STOP_GRACE`, and
    `AgentController::new`, as the constructors of the other controllers are.
  - A test of `session/channels.rs` showed a channel refusing a component it does not ask for with
    the echo tool, an agent's kind; it does so with the session's command `/help` now.
- **2026-10-15** — Step 2, the move: `caelumora-agent` is a crate of the workspace. Its modules
  came over by `git mv`, so their history follows: `tool`, `prompt`, `llm`, `compaction`,
  `component_impl`, `message`, `external`, `actor` (`agent`, `external_agent`, `body`, `intake`,
  `commands`, `run_control`), and at the top `turn`, `work`, `memory` and `notes` - no longer under
  `agent/`, so that a turn is `caelumora_agent::turn::Turn` -, `plugin` (once `agent_plugin`),
  `error` (`agent_error`), `command` (`agent_command`), `testing` (`agent_testing`, behind its own
  `test-support`) and `channel_tests` (once `channel/tests/agents`). The plugins whose components
  serve an agent's facets - skills, artifacts, MCP, A2A, Ollama - and the app and the TUI depend on
  it; skills and artifacts with their layer `runtime`. `aho-corasick`, `regex` and `meval` are its
  dependencies, no longer the runtime's.
  - The security suite is cut in two (point 8): the 50 tests of the runtime stay in
    `caelumora-runtime/tests/security_suite.rs`, the 47 of agents are
    `caelumora-agent/tests/security_suite.rs`, with the helpers only they use; the four both use
    (`checks`, `file`, `read`, `workspace`) are `caelumora_runtime::testing`.
  - Found by building the runtime without `test-support`: the commit before had closed the
    `impl` of `AgentContextView` too early, so that its methods were behind the feature; the
    workspace's tests, which build with every feature, did not show it. Fixed here, and every crate
    is built on its own now as well.
  - The documents (point 9) name the crate: ARCHITECTURE.md, README.md, AGENTS.md, SECURITY.md,
    PERMISSION-CHECKS.md, PLUGINS.md, COMPONENTS.md, COMMANDS.md, CODING-GUIDELINES.md.
  - Measured: 230 unit tests and 50 of the suite in the runtime, 258 and 47 in the agents' crate.
- **2026-10-15** — After the move, at the maintainer's request and on their acceptance of the
  proposals: `actor/agent/mod.rs` is `actor/agent.rs`, and `component_impl` is `components`
  (COMPONENTS.md). The crate is `caelumora-agents`, plural like `caelumora-skills` and its
  `AgentsPlugin`. `actor`, `memory` and `notes` are crate-internal: no other crate uses them, and
  an agent's actor is reached only through the kinds the plugin registers. The facet modules stay
  public, `message` among them, since a plugin may serve its stages. The runtime no longer
  depends on `tokio-stream`, which it did not use. Three proposals of the same review are issues of
  their own: [371](371-tool-executor-construction.md), [372](372-runtime-test-actors.md) and
  [373](373-agent-test-worlds.md).
