# 355 — The two agent actors and `AgentCommands` carry the same half of an agent three times

**Status:** done
**Area:** Code structure

## The problem

[211](211-turn-and-agent-work-structure.md) gave both kinds of agent one `Answering`, one
`catch_up` and one `AgentChannelState`; [269](269-agent-commands.md) added `AgentCommands` as what
both share of commands. What each of them holds of the agent is still written three times:

| Field | `AgentActor` (`actor/agent/mod.rs:45`) | `ExternalAgentActor` (`actor/external_agent.rs:74`) | `AgentCommands` (`actor/commands.rs:29`) |
|---|---|---|---|
| `agent_id`, `agent_controller` | yes | yes | `agent` |
| `actor_runtime` | yes | yes | `runtime` |
| the built context, `limiting_roles` | `context` | `context` | `limiting_roles` |
| `perceive_in_processors` | yes | yes | - |
| `context_provider` | yes | yes | - |
| `agent_state`, `views` | yes | yes | yes |
| `built_from` | yes | yes | - |
| `state: Mutex<AgentChannelState>` | yes | yes | (given) |

And what is done with them is written twice:

- `security_context()` (`actor/agent/mod.rs:109`, `actor/external_agent.rs:155`), and once more
  in `AgentCommands::carry_out` (`actor/commands.rs:103`);
- `id`, `name`, `components`, `commands`, `catch_up` of the `Actor` impl;
- `announce(state)` in its own contexts (`:280`, `:274`);
- the loop "carry out the command where it came, take in the rest of the batch"
  (`actor/agent/mod.rs:362`, `actor/external_agent.rs:300`);
- storing the channel state with a warning: a method in `AgentActor` (`:262`), six inline copies in
  `ExternalAgentActor`;
- both constructors: load the agent and its attributes, derive the context from the controller,
  build the container, load the channel state, get the state handle, the views, `AgentCommands`.

The free functions of `actor/intake.rs` take the same things as five or six parameters each
(`take_in`: state, received, rx, processors, security, elsewhere; `catch_up`: state, visible, own,
processors, security, runtime), because nothing owns them.

## Why it matters

The comments say "the same for both kinds of agent" at every one of these places, but nothing in the
code says it: the invariant is kept by reading two files side by side. Every change to how an agent
hears, stores or announces is made twice, and a third holder of the same fields
(`AgentCommands`) already exists because the actors could not lend theirs.

## Considerations

- **One type for what both share**, in `actor/`, say `AgentCore` (name open): the fields of the
  table and the methods `security_context`, `name`, `perceive`, `take_in` with the command loop,
  `catch_up`, `store_channel_state` (with the warning), `announce`. `AgentActor` keeps
  `llm_client`, the compaction components, `tool_executor`, `reasoning_visibility`, `RunControl`;
  `ExternalAgentActor` keeps `remote`, `pending_prompts`, `processing`, `current`, `ended`. The
  `Actor` impls forward `id`, `name`, `components`, `commands`, `catch_up` to it. `AgentCommands`
  takes a `&AgentCore` where it is called instead of its own copies, or becomes methods of the core.
  The functions of `intake.rs` become its methods; the test of `take_in` builds a core with a
  detached runtime.
- The command loop differs in one line - the `AgentActor` announces `CarryingOutCommand` only when
  awake, the external agent always through `go_to` -: the core takes a closure or the two actors
  wrap the loop's step; to be seen in the code.
- **Not taken:** a trait over both actors with default methods - the fields are the duplication,
  and a trait cannot hold them.
- An architecture decision: a new type with a responsibility of its own ("what an agent is in a
  channel and how it hears"), to be discussed before it is built.

## Done when

- No field of the table and none of the listed methods exists in more than one type.
- `actor/intake.rs` has no function with more than three parameters, or none at all.
- No behaviour changes; the channel tests of agents and external agents pass unchanged.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
- **2026-10-14** — Done. The type is called `AgentBody` (`actor/body.rs`), the maintainer's choice
  over `AgentCore`, which names the structure rather than the responsibility, and over
  `AgentPresence`, which reads as a status: both actors share the body that hears, holds its state
  and carries out commands, and differ in their mind - a model run here, or the remote agent. It
  holds the fields of the table - the agent's controller, the runtime, the context as built, the
  perceive processors, the context provider, the agent's state, the views, what it was built from
  and the `AgentChannelState` - and `security_context`, `id`, `name`, `components`, `turn`, `work`,
  `current_context`, `store` (with the warning) and `announce`. `AgentCommands` is gone: its
  executor and the profile are fields of the body, `commands.rs` an `impl AgentBody` with
  `command_executor`, `visible_commands` and `carry_out`, which builds its turn through
  `AgentBody::turn` - so `Turn::of` is written once, as is the context provider's call and its
  error. The functions of `intake.rs` are methods of the body, none with more than three
  parameters; what a change the transcript did not take is handed to is a field (`hand_over`,
  `tell` for the external agent). `AgentBody::build` does what both constructors did; the
  `AgentActor` sets the profile of its command turns from its model (`with_profile`), the external
  agent its `hand_over` (`handing_over`). Both constructors now take the channel's controller,
  so `create_actor` calls them alike. The tests of `intake` build a body without a session
  (`AgentBody::detached`); the test of `perceive` moved there from the external agent.

  Not done as considered: the loop "carry out the command where it came, take in the rest" stays
  in each actor (five lines each). What differs in it is how each shows where it stands - the
  `AgentActor` through its `RunControl`, only when awake, announcing `Idle` with its next step;
  the external agent through its own `processing`, always and at once. Sharing the loop would have
  needed a callback or a trait for that one step; the cause is that where an actor stands, and what
  `cancel` and `stop` asked of it, is built twice (`RunControl`, and `processing`, `current` and
  `ended`), which is a refactoring of its own. No behaviour changes; the warnings of a failed store
  read alike now.
