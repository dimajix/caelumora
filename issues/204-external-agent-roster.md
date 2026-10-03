# 204 — An external agent is not told who is in its channel

**Status:** done
**Area:** Integrations

## The problem

An agent of our own learns who is in its channel from `channel_members_prompt_provider`
([176](176-participants-in-system-prompt.md)): its own name, the others with their `@` handles and
introductions, and that a member is addressed by mentioning its handle. An external agent
([179](179-external-agents-via-a2a.md)) learns none of this. A `PromptProvider` contributes to the
context a `Turn` builds for a model, and an external agent has neither: its components are its
`ExternalAgent` (such as `a2a_agent`) and perhaps `PerceiveInProcessor`s. When it is asked,
`ExternalAgentActor` sends the remote agent only what was said since it was last asked, as
`Said { sender, text }` (`caelumora-runtime/src/external.rs`).

So the remote agent knows the names of those who spoke, but not who else is there, under which
handle, nor that a mention exists. Since [45](45-agent-mentions.md) a mention written by an external
agent makes the mentioned agents respond - but it would have to guess the notation and the handles.

## Why it matters

- Delegation between agents, the reason for [45](45-agent-mentions.md), is open to our own agents
  only; an external agent can be addressed but cannot address anyone.
- It cannot tell a person from an agent, nor whom it is talking to when someone joins who has not
  spoken yet.

## Considerations

### What is sent, and how

1. **A leading part of the request.** `OutboundMessage` gains the roster as text of its own (e.g.
   `members: Option<String>`), which `a2a_agent` sends as a first part of the message. It is sent
   with the first request of a conversation and again whenever the roster changed since it was
   last sent; the external agent's channel state remembers what it sent. Every remote agent reads
   it, since it is part of what it is given.
2. **Metadata of the A2A message.** The roster travels as structured metadata. Cleaner, but only a
   remote agent that knows to read it benefits; to most it is invisible.
3. **Every request.** Simplest, but repeats the roster in the remote conversation each time.

Option 1 looks most useful; whether the part is plain text, or text plus metadata (option 2 on
top), is to be decided.

### Where the roster is made

The text of the roster lives in `channel_members_prompt_provider` (`render`, `named`, `one_line`),
built from a `Turn`. An external agent needs the same text from its channel without a turn. The
roster - reading the members and rendering them - should move to one place both use, rather than be
written a second time; whether that is a function over the channel's members, a method of the
channel's view, or a component facet that the external agent may be configured with (so that it can
be switched off, as for our own agents), is the architecture question of this issue.

### What leaves the system

Names, handles and introductions are public (176), so sending them out needs no new check. It is
still something the remote agent was not given before; whether it is sent should be configurable
per external personality, as the prompt part is per personality.

## Done when

- An external agent's remote agent is told its own name, the others with their handles and kinds,
  and how to address one, when a conversation begins and when the roster changes.
- The roster is made in one place for our own and for external agents.
- It can be switched off per personality.
- CHANNELS.md (external agents) and COMPONENTS.md say so.

## History

- **2026-10-03** — Issue created, after asking whether an external agent gets the roster from
  `channel_members_prompt_provider`. It does not: prompt providers act on a `Turn`, which an
  external agent does not have.
- **2026-10-03** — Checked what A2A offers
  ([specification](https://a2a-protocol.org/latest/specification/), v1.0): nothing for participants.
  A conversation has two sides, client and server; a `Message` has only `role` (`ROLE_USER` or
  `ROLE_AGENT`), so who in the client's channel spoke cannot be said but in the text, as `Said` does
  today. `contextId` groups a conversation, it lists no members. What there is: free `metadata` on
  messages and tasks, data parts next to text parts, and extensions - named by a URI, offered in the
  agent card, named in `Message.extensions` by a message that uses one, meant to give metadata a
  fixed type for a purpose. No published extension for rosters or group conversations was found.
  Consequence for the options above: the leading text part (option 1) is the only form every remote
  agent reads and stays the base; option 2 would be an extension of our own, with the roster as a
  data part or in `metadata`, sent only to a remote agent whose card offers it - useful only for
  agents built for it.
- **2026-10-03** — Decided, after a discussion with the maintainer:
  - **What is sent.** The roster as a leading text part of the request (option 1); A2A has no
    system messages, so it is part of the user message, marked as the channel's frame. It is sent
    when a conversation begins and when the members change: then the whole list again, with who
    joined and who left - naming the change explicitly helps a model take it in.
  - **Where it is made.** In `channel_members_prompt_provider`, which an external personality is
    configured with as an agent's is - so it can be switched off per personality. A parameter, not
    a second kind, switches it to telling changes only: it keeps what it named last and contributes
    nothing while nothing changed. It starts empty, so after a restart the whole list is sent again,
    which is accepted. Split into a kind of its own only if the two modes share too little.
  - **How it reaches the remote agent.** `ExternalAgentActor` asks the prompt providers of its
    personality before every request and sends their parts first; positions mean nothing there and
    are ignored. What a failed send did not deliver is to be sent with the next request.
  - **What the providers are given.** Not a `Turn`, which an external agent does not have, but a
    lean replacement both kinds of agent have - the read-only part of a piece of work: who works,
    in which contexts, bounded how, and the agent's states to read from. Not a type per facet:
    `MessageProcessor` lost its `Turn` the same way once the external agent needed it. As a first
    step, the duplication between `TurnState` and `Work` is removed: `TurnState` holds a `Work`.
- **2026-10-03** — Step 1: `Work` moved to `caelumora_model::work` and `TurnState` holds one
  (`TurnState::work`, stored flattened, so stored turns keep their form). `Work` took over what
  `TurnState` had of it: `seen` as an `Option` (a turn stored before it was kept counts as having
  seen its channel), `context` as a `ContextRef` (an external agent's is empty until sent), and
  `label`, `see`, `see_classified`, `see_message`, `record_knowledge`. No change of behaviour, but
  one: an external agent's task stored with a `Work` but before issue 205 recorded its answer with
  no context, and now with an empty one.
- **2026-10-03** — Step 2: `AgentWork` (`caelumora-runtime/src/work.rs`) is the read-only part of
  a piece of work - the agent, its `Work`, its `AgentChannelState`, its `AgentStateHandle` and its
  views - and what a `PromptProvider` is given instead of the `Turn` (`Turn::work`). The reading
  helpers `AgentNotes::current`/`digest` and `AgentMemory::recalled`/`digest` take it; what stages
  changes keeps the `Turn`. `Turn`'s reading accessors (`session`, `classification`, `label`,
  `channel_id`, `session_id`, `security_context`) answer through it. No change of behaviour. An
  `ExternalAgentActor` can load the agent's `AgentStateHandle` as an `AgentActor` does
  (`ActorRuntime::agent_state`, which only reads), so it can build one too - step 3.
- **2026-10-03** — Step 3: an `ExternalAgentActor` asks the `PromptProvider`s of its personality
  (`crate::context::prompt_parts`, which `state_context_provider` uses too, each by its fail
  policy) with every request, and gives the remote agent their parts first
  (`OutboundMessage::prompts`; `a2a_agent` sends each as a text part ahead of what was said).
  Beyond what was decided: a part is given again only when its text changed since it was given -
  without that, a fixed part such as `static_system_prompt_provider`'s would be repeated with every
  request. What a failed send did not give stays pending and goes with the next request; all of
  this is kept in memory, so after a restart every part is given again. The parts are kept by
  digest and recorded in the context of the call (`AgentChannelState::keep_prompt`, which `Turn`
  uses too), and the work has seen them and the knowledge they hold. Not tested yet end to end:
  the roster of an external agent in a running session - the channel tests run without one, so the
  mechanics are tested with a fixed and a changing part; to follow with step 4.
- **2026-10-03** — Step 4, done: `channel_members_prompt_provider` takes the parameter
  `changes_only`. With it, the component keeps the roster it told last (built for each actor, so
  for each channel) and tells it again only when it changed, followed by who joined and who left -
  told apart by actor, so a new name is no new member. A test runs an external agent in a session,
  is told alice, then the whole roster with "Joined: bob" after bob joins. AGENT-TURN.md,
  CHANNELS.md and COMPONENTS.md say so. All of "Done when" holds; what leaves the system is public
  (176), and whether it is sent is the personality's choice of components.
