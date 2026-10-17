# 214 — The prompt logic is split between the context provider and a free function

**Status:** done
**Area:** Code structure
**Summary:** What an agent is given besides the conversation is decided in a component, a free function and, for an external agent, its actor - and the component's facet writes into the turn instead of answering.

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
- **2026-10-03** — Step 2: `ContextProvider::current_context` takes an `AgentWork` and returns
  `(Vec<PromptPart>, Vec<KnowledgeRef>)`; the `AgentActor` sets them with `Turn::record_knowledge`
  and `Turn::set_context`, which stays as it was. `ComponentImpl::apply_fail_policy` applies a
  component's fail policy to a result of its facet - `None` for a part left out - and both the
  message processors' stages and the prompt providers use it; a failure that fails closed is now
  named "component '...' failed" for both, and the warning reads alike. `TurnContextProvider` asks
  its providers itself again; `prompt_parts` remains only for `ExternalAgentActor`, until step 3.
- **2026-10-03** — Before step 3, decided with the maintainer, in place of what "What the remote
  agent was given" above says: giving a part again is the actor's responsibility, not the context
  provider's - the actor knows what a prompt is anyway, it sets a turn's. So the component has no
  state: `PromptPart` gains `source`, the component a part came from alone, and the actor records
  by it what the remote agent was given last. The alternative - the actor only storing the context
  of its last call, the component keeping all the rest - was weighed and not taken. The default of
  an external agent's `ContextProvider` is given by its actor, for its container alone
  (`ComponentContainer::with_defaults`), rather than by the registry per kind of personality.
- **2026-10-03** — Step 3, done: `external_context_provider` (`ExternalContextProvider`) asks its
  prompt providers by their fail policy and returns each part on its own, with its `source`, role
  user, unplaced. `ExternalAgentActor` takes its `ContextProvider` from its container, built with
  that default; it keeps the parts not given yet by `source` - else by their place - in memory,
  gives those, and after a send that reached the remote agent records their digests in
  `RemoteState::given_prompts` (stored; `RemoteState::give`, `has_given`). Another conversation
  opened by the remote agent forgets them (`RemoteState::converse_in`). The work has seen every
  part returned and the knowledge they hold. `prompt_parts` and `PromptDelivery` are gone. Tests:
  the component, a container's own defaults, `RemoteState`, and that after a restart only a part
  that changed is given; the tests of 204 hold as they were. CHANNELS.md, COMPONENTS.md and
  CONFIGURATION.md say so.
