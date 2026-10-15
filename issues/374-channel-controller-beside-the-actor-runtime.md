# 374 — Every actor is given its channel's controller beside its `ActorRuntime`

**Status:** done
**Area:** Code structure

## The problem

Since [370](370-agent-logic-in-a-crate-of-its-own.md) the `ActorRuntime` is the actor's interface
to its channel: what an actor needs of the channel and does not find there is added there
(CODING-GUIDELINES.md, *Runtimes and their entities*). Yet every actor is still given its
channel's controller as a parameter of its own, beside that runtime:

- `UserActor::new(channel_controller, user, actor_runtime)`
- `ApproverActor::new(channel, approver, actor_runtime)`
- `AgentKinds::build(channel, agent, personality, runtime)`, and so the agents' plugin's
  `AgentActor::new` and `ExternalAgentActor::new`
- `ToolExecutor::new(providers, channel, runtime)`, which kept the parameter in
  [371](371-tool-executor-construction.md) for this reason

The reason is the order in which a channel is built: `ChannelLoop::new` makes the
`ChannelRuntime` - from which the actors' runtimes are made - before the channel's controller
(`ControllerManager::new_channel_controller`), and then builds the actors with both
(`ChannelLoop::create_actor`). The session's controller, by contrast, is the `ChannelRuntime`'s
and reaches the actor through `ActorRuntime::session_controller`.

## Why it matters

- Two ways into the same channel can disagree: nothing keeps a caller from passing the controller
  of one channel and the runtime of another, as the executor's tests did before 371.
- Every constructor of an actor, and every kind of agent a plugin registers, carries the
  parameter, and with it the question what an actor may do with the controller that it should not
  do through its runtime.
- The session's controller and the channel's reach the actor in two different ways for no reason
  of their own.

## Considerations

- **The channel's controller in the `ChannelRuntime`:** `ChannelLoop::new` (or the session, in
  `SessionLoop::channel_runtime`) makes the controller first and gives it to the channel's runtime,
  as the session's; `ActorRuntime::channel_controller` hands it out like `session_controller`. The
  constructors of the actors, `AgentKinds::build` and `ToolExecutor::new` lose the parameter. To
  be weighed: who makes the controller then - the channel or its session -, and whether the
  `ChannelLoop` keeps its own handle or reads it from its runtime.
- **What an actor may do with it:** the controller can change the stored channel; whether the
  `ActorRuntime` hands it out whole or offers what the actors need of it (as it does for the
  classification and the attributes) is part of the decision.
- **Leave it.** The parameter is passed in one place (`create_actor`) and the disagreement is not
  produced anywhere today; the cost is one parameter per actor.

## History

- **2026-10-15** — Done, by the maintainer's decision, as the first consideration has it: the
  session makes the channel's controller as it makes the channel's runtime
  (`SessionLoop::channel_runtime(channel)`), and the `ChannelRuntime` holds it as it holds the
  session's. `ChannelLoop::new` takes the runtime alone and builds the channel from that
  controller, keeping a handle of its own taken from the runtime once, as it takes the services.
  The `ActorRuntime` hands the controller out whole (`channel_controller`), as it does the
  session's: what changes the channel is asked by the controller in the caller's name anyway, so
  a narrower offer would protect nothing. The constructors of the actors, the factories of
  `AgentKinds`, `AgentBody::build` and `ToolExecutor::new` lost the parameter, and
  `ChannelRuntime::new_actor_runtime` the ids of the session and the channel, which it reads from
  the controllers. As a step of its own, `ActorRuntime::channel_classification` and
  `channel_attributes` read the controller instead of the channel's loop and are no longer
  optional: once the channel is gone, what ends a turn is bound by the channel as it stood last,
  as the session's controller guards its tool calls; the agents' branches for a channel that is
  not there went away, and `AgentActor`'s `TurnFlow` became `lowered`, as the external agent's
  already was. The tests build a channel's runtime with its channel (`outside_a_session`,
  `bare_channel`, `detached_actor_runtime_in`), and a test's actor runtime is made by its channel
  (`testing::actor_runtime_in(channel, actor)`), so that a test no longer builds a second
  controller beside the runtime's.
