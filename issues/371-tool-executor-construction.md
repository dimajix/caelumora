# 371 — `ToolExecutor::new` takes seven parameters, five of which its caller reads from the agent's runtime

**Status:** done
**Area:** Code structure

## The problem

Since [370](370-agent-logic-in-a-crate-of-its-own.md) the `ToolExecutor` holds the agent's
`ActorRuntime`, through which it puts a call's questions to the users of the channel. Its
constructor still takes everything else one by one, as it did when it had no runtime:

```rust
pub(crate) fn new(access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>, providers: Vec<ComponentImpl<dyn ToolProvider>>, system_rules: Arc<[ApprovalRule]>, session: Arc<SessionController>, channel: Arc<ChannelController>, runtime: ActorRuntime) -> ComponentResult<Self>
```

Its one caller in production, `AgentActor::new` (`caelumora-agents/src/actor/agent.rs`), reads
four of them from that same runtime - `access_policy()`, `confidentiality()`, `approval_rules()`,
`session_controller()` - and passes the channel's controller it was given itself. Only the
providers are the executor's own; the rest says where the agent works, which the runtime already
says.

## Why it matters

Seven parameters, five of which repeat what the seventh holds, can disagree with it: nothing keeps
a caller from passing the rules of one session and the runtime of another. The executor's tests
do exactly that on purpose (`tool::tests::guarding_under`): they build a session and a channel
controller of their own, with the rules of the test, and a detached runtime
(`detached_actor_runtime()`) that has neither. That is why the parameters were left as they are -
the tests have no runtime to give that holds what they need.

## Considerations

- **`ToolExecutor::new(providers, channel, runtime)`**, the policy, the confidentiality, the
  system's rules and the session's controller taken from the runtime when it is made - as the
  executor keeps them, the rule that a type takes its services when it is made and keeps them
  holds. The channel's controller stays a parameter until `ActorRuntime` offers it; whether it
  should is a question of its own (issue 144's rule, and its exception for the crate boundary in
  CODING-GUIDELINES.md).
- **The tests need an `ActorRuntime` that holds what they give it.** A fixture of the runtime's
  test support - a runtime of a session and channel of the test's, with its policy and its rules -
  would serve these tests and any other of the agents' crate that builds a part of an agent
  without a running channel. Without it, the change only moves the problem into the tests.
- **Leave it.** The constructor is crate-internal and has one caller; the cost is a long line and
  a disagreement nothing produces today.

## History

- **2026-10-15** — Done, as the first two considerations together, by the maintainer's decision:
  `ToolExecutor::new(providers, channel, runtime)` takes the policy, the confidentiality, the
  system's rules and the session's controller from the agent's runtime when it is made, and keeps
  them. The channel's controller stays a parameter: the `ActorRuntime` does not hold it, since
  `ChannelLoop::new` makes it after the channel's runtime and hands it to every kind of actor;
  changing that is a question of its own. For the tests, the runtime's two fixtures are generalized
  rather than joined by a third: `test_runtime_under(access_policy, confidentiality, config)` builds
  a runtime under the test's policies and configuration, the system's rules among it, and
  `detached_actor_runtime_in(runtime, session, channel)` places an actor in a session of the
  test's that does not run; `test_runtime()` and `detached_actor_runtime()` are these with the
  defaults. `ChannelRuntime::outside_a_session` takes the session instead of its id
  (`bare_session` for one that holds nothing else). The executor's tests build their runtime from
  the rules of their places and read the session's controller from it, so that policy, rules and
  session can no longer disagree.
