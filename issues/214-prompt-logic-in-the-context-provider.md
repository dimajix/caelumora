# 214 — The prompt logic is split between the context provider and a free function

**Status:** in progress
**Area:** Code structure

## The problem

Until [204](204-external-agent-roster.md), `state_context_provider` held all of what an agent's
model is given besides the conversation: it asked every `PromptProvider` of the agent - each by its
component's fail policy - joined the parts per position, placed them in the transcript and wrote
them and the knowledge they hold into the turn. 204 gave an external agent prompt parts too, and to
do so took the asking of the providers out of the component into a free function,
`crate::context::prompt_parts`, which the component and `ExternalAgentActor` both call. The external
actor in turn keeps what of each part it gave (`PromptDelivery`), in memory.

So the prompt logic of an agent no longer lives in one place: a part is in a free function, a part
in a component, and for an external agent a part in its actor. Besides, the component's name says
nothing of what it is: `state_context_provider` assembles the context of a turn.

The facet makes it worse than it needs to be: `ContextProvider::current_context` takes a `&mut Turn`
and writes into it (`Turn::set_context`, `Turn::record_knowledge`), so it cannot be what an external
agent asks, and what it does to the turn is not visible in its signature.

## Why it matters

- One owner per concern: what an agent is given besides the conversation should be decided by its
  `ContextProvider`, wholly, for both kinds of agent.
- A component that only reads is easier to test and cannot do to the turn what it should not.

## Considerations

### Using the turn's component for external agents too

Considered and rejected: it joins the parts of a position into one text, places them in the
transcript and gives them roles for a language model. An external agent needs each part on its own
- one is given again only when it changed, and one a failed send did not give goes with the next -
and has no positions, no placement and no roles. What the two would share is asking the providers,
nothing more.

### Two components, one facet

Each kind of agent has one `ContextProvider`, which holds its prompt providers and asks them itself:

- `turn_context_provider` (today `state_context_provider`), the default of an agent run here, as
  today;
- a component of its own for external agents, their default, which returns each part that the remote
  agent does not have yet, unjoined.

`ContextProvider::current_context` becomes a question: it is given the work to read
(`AgentWork`, 204) and returns what it assembled, `(Vec<PromptPart>, Vec<KnowledgeRef>)`; the
caller sets it - for a turn, `Turn::set_context` and `Turn::record_knowledge`, nothing else. That it
does nothing else is guaranteed by the type: `AgentWork` only reads.

Asking one provider by its fail policy is not prompt logic but a property of a component;
`actor_impl` applies the same policy to message processors. It becomes a generic method of
`ComponentImpl<T>`, which both use; `prompt_parts` goes.

### What the remote agent was given

The external component must know what the remote agent has, without being told by its caller
whether a send succeeded. Split:

- the actor records, after a send that succeeded, the digest of each part it gave, by the id of its
  prompt provider, in `RemoteState` - as it records the conversation and starts its transcript anew;
- the component keeps the last part each provider gave that was not empty, and returns it while its
  digest is not the one recorded - read through `AgentWork`.

A send that failed records nothing: the part stays due, even when a provider that tells only changes
has nothing new the next time. After a restart a fixed part is not given again, since what was
given is stored; the roster is given once more in full, which 204 accepted. A new conversation
clears what was recorded.

### Old data

Stored data and configurations from before need not be kept working: the component kind is renamed
without an alias.

## Done when

- `state_context_provider` is `turn_context_provider`, everywhere.
- `ContextProvider::current_context` takes an `AgentWork` and returns parts and knowledge.
- The fail policy of a component is applied by one method of `ComponentImpl`, for prompt providers
  and message processors alike.
- An external agent has its own `ContextProvider`, its default; `prompt_parts` and
  `PromptDelivery` are gone, and what was given is recorded in `RemoteState`.
- AGENT-TURN.md, CHANNELS.md, COMPONENTS.md and CONFIGURATION.md say so.

## History

- **2026-10-03** — Issue created after 204, from the maintainer's wish to have the prompt logic in
  one place again, and their proposal that the context provider return its parts instead of setting
  them. The options and the decision above were discussed with the maintainer before. Step 1 begun: the rename.
- **2026-10-03** — Step 1: `state_context_provider` is `turn_context_provider`
  (`TurnContextProvider`, `component_impl/turn_context_provider.rs`), in the code, the default of
  the facet, `examples/config.yaml` and the docs; no alias. Older issues keep the old name, as the
  record they are.
