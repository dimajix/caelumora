# 374 — Every actor is given its channel's controller beside its `ActorRuntime`

**Status:** open
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
