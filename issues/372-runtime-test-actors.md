# 372 — The runtime's tests write four actors of their own beside the scripted agent

**Status:** done
**Area:** Code structure
**Summary:** The runtime's tests implement `Actor` four times beside the scripted agent, each writing the trait out again for a test or two.

## The problem

With [370](370-agent-logic-in-a-crate-of-its-own.md), the runtime's test support got an actor of a
kind of agent with nothing of an agent's logic: `testing::ScriptedAgent`, registered by the plugin
`ScriptedAgents`, which answers or hangs as its `Script` says, tells where it stands and keeps
what it heard and whether it was woken. Beside it, the runtime's tests still implement `Actor`
four more times, each for a test or two:

| Actor | Where | What it does |
|---|---|---|
| `Sleeper` | `channel/tests/mod.rs` | asleep; tells whether it was woken or cancelled |
| `TestActor` | `moderator/policy_moderator.rs` (tests) | in the state it is given; does nothing |
| `SaysOnce` | `channel/run.rs` (tests) | says one message one hop into a chain, then waits |
| `Says` | `channel/run.rs` (tests) | sends the envelopes it is given, as they are written, then waits |

Each writes the trait's methods out again - `id`, `name`, `event_filter`, `run`, `state`,
`wakeup`, `stop` - and the first two keep what a scripted agent keeps too.

## Why it matters

Five implementations of one trait in one crate's tests are five places to change when the trait
changes, and a reader of a test has to read its actor to know what it stands for. Some of the
differences have a reason: `Says` sends envelopes in another's name on purpose, which no scripted
agent should be able to do, and `TestActor` is given to a moderator without a channel.

## Considerations

- **`Sleeper` and `SaysOnce` as scripts** of `ScriptedAgent` - `Script::Sleeps`,
  `Script::SaysAtStart(message, hops)` -, adopted as the tests adopt them now. `ScriptedAgent`
  already keeps whether it was woken; whether it was cancelled is one flag more.
- **`TestActor` stays**, or becomes a `ScriptedAgent` built without a channel, if its constructor
  allows it: the moderator only asks an actor's id and state.
- **`Says` stays**, named as what it is - an actor that forges envelopes, for the test that the
  channel refuses them - so that nobody mistakes it for a fixture to reuse.
- **Leave it.** The actors are small and private to their tests; the cost is the repetition.

## History

- **2026-10-15** — Done, by the maintainer's decision, otherwise than the first consideration: five
  implementations became three, each with a task of its own. `Sleeper` and `TestActor` were the
  same actor - an id, a name and a state, a `run` that does nothing - and are one now,
  `testing::StandIn` (`#[cfg(test)] pub(crate)`, as only the runtime's tests use it): in the state
  it is put in until it is woken, keeping whether it was woken or cancelled. Making them scripts of
  `ScriptedAgent` was not taken: a scripted agent is built by the runtime from a stored agent,
  which the tests of routing and of finding agents by their names would have had to store for
  nothing, and `TestActor` stands for users too. `SaysOnce` was `Says` with one envelope of one hop
  written by hand, and is that now; `Says` is `Forger`, private to the tests of `channel/run.rs`,
  documented as an actor that sends what no actor of the runtime can. Building the cut-off chain
  of real scripted agents instead was weighed and not taken: it needs stored agents and a builder
  of its own for a hop count the test writes directly. CODING-GUIDELINES.md says which actor a
  test uses.
