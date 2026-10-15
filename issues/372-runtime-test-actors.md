# 372 — The runtime's tests write four actors of their own beside the scripted agent

**Status:** open
**Area:** Code structure

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
