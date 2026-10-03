# 205 — What an external agent was given for a message cannot be shown

**Status:** open
**Area:** Integrations

## The problem

For a message of an agent run here, the web UI shows what the agent saw when it produced it
(`ContextPanel`, `GET …/actors/{aid}/messages/{mid}/context`, issues 06 and 47): its transcript up
to the message, the prompt parts, the tools, what it read. For a message of an external agent
([179](179-external-agents-via-a2a.md)) there is nothing: `ExternalContext::context_of`
(`caelumora-runtime/src/view/context.rs`) answers every message with `NotRecorded`, so the panel
offers the context and then fails.

The external agent's transcript does not answer the question either. It is every message ever
given to the remote agent, in order (`ExternalAgentActor::exchange` extends it with what it sent),
and nothing ties one of the agent's answers to the call it answered.

## Why it matters

- What an external agent is given is what leaves the system. Who may read an agent's context should
  be able to see, per answer, exactly what went out for it.
- An answer of the remote agent can only be judged against what it was given - which, unlike an
  agent run here, is not the whole conversation but what was new since it was last asked.

## Considerations

### What the context of an external agent's message is

Not the same as for an agent run here: there, the context of a message is everything the model was
given for it - the transcript so far, prompt parts and tools - since a model keeps nothing between
calls. A remote agent keeps its conversation itself; each call gives it only what was said since the
last one. So the context of a message of an external agent is **exactly the messages sent in the
one call that produced it**: the `said` of that `OutboundMessage`, as they passed the perceive
processors ([196](196-perceive-processors-write-state.md)), and which task it answered, if it
answered a question back. No prompt parts, no tools, no knowledge.

Cases to cover:

- **An answer** to a call: the messages of that call.
- **A question back** (`input-required`) and the answer to it are two calls; each message belongs
  to its own.
- **A call with nothing new**, which only answers a task's question with what was said meanwhile:
  the messages of that call, perhaps none.
- **What a task says unasked** (`ExternalAgentActor::speak`, after the request it was given for has
  ended): the call that began or last took up the task. `OpenTask` keeps that call's answer context
  (`requested`, issue 45) already; it would also have to keep, or point to, what was sent.

### How it is recorded

The transcript stays what it is - all that was given, in order. What is new is a record per call,
and per message of the agent which call it came of. Options:

1. **A reference into the transcript**, as for an agent run here: with each message of the agent
   in its history, the digest of the transcript before the call and how many entries the call
   added. Resolving it gives exactly the call's messages, and the record stays small. It needs a
   reference type of its own, or `ContextRef` used with no prompts, tools and profile - which
   says less than it seems to.
2. **The messages themselves** with each answer. Simple and independent of the transcript, but
   stored twice.

Option 1 fits the way contexts are recorded today; whether `ContextRef` or a type of its own is the
architecture question of this issue. `AgentContextView::context_of` then answers for both kinds of
agent, and `ResolvedContext` already has room for messages without prompts.

### What the UI shows

`ContextPanel` shows the messages that were sent, under a heading that says they are what was sent
in this call - not "what the model saw" - and omits prompt parts, tools and knowledge, or says
there are none. Which task the call answered may be shown too.

## Done when

- For every message of an external agent, its context is the messages sent in the call that
  produced it - also for a question back, its answer, and what a task says unasked.
- The REST API returns it as for an agent run here, and the web UI shows it.
- AGENT-TURN.md or CHANNELS.md (external agents) and WEB-UI.md say how the context of an external
  agent differs from that of an agent run here.

## History

- **2026-10-03** — Issue created. Decided with it: the context of an external agent's message is
  the messages of the one call that produced it, unlike an agent run here, whose context is all its
  model was given.
- **2026-10-03** — Decided: as much of the existing infrastructure as possible is reused. The
  target, perhaps, is one state for agents run here and external agents. What is there to reuse:
  - `ExternalAgentChannelState::history` is a `MessageHistory` already, whose `Said` entries carry
    an `Option<ContextRef>` - the external agent records `None` today. It records a `ContextRef`
    instead, and `AgentContextView::context_of` finds it as `InternalContext` does.
  - `ContextRef` says what is needed: `transcript`, the digest of the transcript including the call,
    and `turn_entries`, how many of its last entries the call added. `prompts` stay empty, `tools`
    `None`. `ContextResolver::resolve` already splits the transcript into what came before and what
    the turn added; for an external agent only the second part is its context. Whether that is
    told by the `profile`, by the kind of agent the resolver is made for, or by a flag of the
    reference, is to be decided.
  - `ContextResolver::of` and `TranscriptView::of` take an `AgentChannelState`; they would take the
    parts they read (transcript, prompts, tool sets, the turn's pending entries) so that an
    `ExternalAgentChannelState` can be resolved too - a step towards one state.
  - Option 2 (the messages stored with each answer) is dropped.

  Merging the two states is issue [206](206-one-agent-channel-state.md).
- **2026-10-03** — Order decided in [206](206-one-agent-channel-state.md): the two states are
  merged first, and the external agent's transcript gets a mark of what was sent. So
  `ContextResolver::of` and `TranscriptView::of` need not take the parts they read, and the context
  of a message is the entries between two marks. This issue follows step 4 of 206.
