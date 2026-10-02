# 179 — External agents cannot take part, only tools and models

**Status:** in progress
**Area:** Integrations

## The problem

Caelumora reaches outside itself in two ways: a language model, which an
agent has as a component (issue [130](130-llm-configuration.md)), and remote
tools, which an `mcp_tool` component discovers on an MCP server. An agent
that lives elsewhere - with its own model, its own tools, its own memory -
cannot be brought in. It can be neither asked for a piece of work by one of
our agents nor seated in a channel beside them.

The Agent2Agent protocol (A2A, v1.0, stewarded by the Linux Foundation) is
the open standard for exactly that: an agent publishes an *Agent Card*
describing its skills, and is given *tasks* over HTTP (JSON-RPC, with
streaming), which run, may ask for more input (`input-required`) and end
with messages and *artifacts*. MCP connects an agent to tools, A2A connects
agents to each other; the two complement each other.

## Why it matters

Specialised agents are increasingly built as services of their own. Without
A2A, every one of them would have to be rebuilt as a personality with tools,
or wrapped as an MCP tool, which loses what makes it an agent: a task that
runs for a while, asks back, and answers in several parts.

## Considerations

### What an external agent is not

An external agent differs from ours in ways that are not a matter of
degree:

- We know only what we sent it; its true context is not ours to see.
- It has no tools of ours - and the tool integration (approval, guarding,
  classification, plans) is a large part of what `AgentActor` is.
- It may speak unprompted, without a `ResponseRequested`.
- It needs no `Turn` on our side; planning, reflecting and resuming a
  cut-off turn are its business.
- The state we keep of it is thinner, and different: what we sent, and the
  protocol's handles (an A2A `contextId`, open task ids) - not a
  transcript or notes.
- It runs on without us; answers may arrive while we are down.

Making it an `AgentActor` whose `LLMClient` is the remote agent was weighed
and rejected: that facet only answers when asked, so an agent that speaks
on its own does not fit, and everything else of the actor would be ballast.
Its nearer relative is the `UserActor`: no turn, no tools, speaks
unprompted, known only by what passed through the channel.

### Decision: an actor of its own, chosen by the personality

- **`ExternalAgentActor`**, a fourth actor beside `AgentActor`, `UserActor`
  and `ApproverActor`. What it owes the channel is what every participant
  owes it: receive envelopes, publish `FullResponseGenerated` (and
  `ResponseStarted`/partials when it was asked), honour `cancel` and `stop`.
  A reply it is asked for becomes a task; a message the remote agent sends
  on its own is published like a user's. `input-required` becomes a message
  into the channel with the task kept open; the next message to it
  continues the task. After a restart the actor reconciles its open tasks
  (`tasks/get`), since they may have finished meanwhile.
- **Identity stays `ActorId::Agent(AgentId)`** with an `Agent` entity: name,
  scope, personality and owner mean the same for an external agent.
  `Participant::Agent`, the templates, the stores, REST and the web UI are
  untouched; whether an agent is external is a property of the agent, not
  of its seat in a channel.
- **`Personality.kind`** says what kind of agent the personality makes -
  internal (today's) or external. Explicit rather than derived from the
  components (which `create_plan_tool` is, see `Personality`): clearer and
  simpler. No check beyond the natural one: an external personality
  without an `ExternalAgent` component fails when its actor asks for the
  facet, with the error naming what is missing; components the external
  actor never reads (tools, prompt parts, `LLMClient`) are not policed - a
  personality put together wrongly does not work, which is as it should
  be.
- **`Member::ExternalAgent`** in the channel's private `Member` enum
  (`caelumora-runtime/src/channel.rs`), derived in `Member::of` from the
  personality's kind, and `create_actor` builds the actor from it. That is
  where "what its actor is built from" is decided today.
- **The connection is a component.** A new facet `ExternalAgent`, of which
  an external personality has exactly one: send a message and stream what
  comes back, cancel a task, list open tasks, and the state the actor keeps
  of it. `a2a_agent` is its first implementation, built like `mcp_tool`:
  URL, authentication from the `SecretManager`, the Agent Card read when it
  is built. The actor knows only the facet; a second protocol is a second
  component kind.

### Decision: cleared at the door, then shown everything

Nothing in the system asks today whether information may leave it: an
`mcp_tool` classifies what comes back, what the agent writes into the
arguments goes out unchecked (issue [180](180-outbound-flow-unchecked.md)).
The external agent makes the question unavoidable, since the channel itself
goes out. Three ways were weighed:

- *A destination with a classification, checked per message* - a new
  `ActionType` asked before every send, what does not flow is not sent, the
  mirror of `withheld`. Finer than needed, and the remote agent has gaps in
  the conversation without knowing.
- *Clearance at join* - the question that exists, `JoinChannel`, with the
  roles the agent is limited to. Sufficient because no message is classified above
  its channel, and raising a channel re-asks `JoinChannel` for everyone in
  it.
- *The owner decides, no rule* - consistent with MCP today; the gap stays.

Decided: **clearance at join**, and then the remote agent is shown
**everything others say in the channel**, as a cleared person is - it keeps
the conversation with us, one A2A `contextId` per channel. Showing it only
what it is addressed with is a matter of volume and cost, a component
parameter at most, not a confidentiality rule.

- **The roles belong to the personality** (`Personality.roles`), as a
  user has roles: an agent has them, not the connection it is reached
  through. They are read where `JoinChannel` is asked - before anything is
  built, for a template's channels before the first store write - from the
  personality, which is loaded there anyway; no component is built and no
  card fetched to answer. Weighed and rejected: a field `roles` on every
  `ComponentDescriptor` (meaningful for one kind only), and `roles` in the
  `params` of `a2a_agent` (the core would have to read one kind's params,
  or build the component at the door).
- **They limit, they do not replace.** `RuntimeManager::roles_of` gives an
  agent its owner's roles, since it must not see more than the person
  responsible for it. An agent is cleared for a channel only if its owner
  is *and* its personality's roles are: `JoinChannel` asked twice, no new
  `ActionType`. A personality can only narrow, so whoever may write one
  cannot widen anyone's clearance through it.
- **None named.** An internal personality that names no roles limits
  nothing - its agents are cleared as their owner, as before. An external
  one that names none is limited to no roles, which sees only what is
  `public`: what is given to it leaves the system, so a forgotten field
  means less, not more (`Personality::limiting_roles`).
- **Internal agents too.** The limit applies to every agent, so an
  internal agent whose model or tools are outside the system - a cloud
  `LLMClient`, an MCP server - can be held back the same way (issue
  [180](180-outbound-flow-unchecked.md)). Where else roles should count is
  issue [181](181-where-roles-decide.md).
- **What comes back** is classified as MCP results are: the component's
  configured `classification` if any, else the channel's. A2A messages
  carry `metadata` through which the remote agent could classify its own
  later - the sibling of issue [139](139-mcp-result-classification.md).
- **A limit, documented and not solved.** Session privacy (`private`,
  `same_owner`) promises that what was said stays in the session. An
  external agent with memory of its own runs on and carries what it heard
  into whatever it does next; we decide whether it is let in, not what it
  keeps - issue [143](143-joining-a-channel-with-foreign-knowledge.md)
  without our side of the control. CONFIDENTIALITY.md is to say so.

### Still open

- **The remote agent as a tool.** Delegation from one of our agents - an
  `a2a_agent` also providing `ToolProvider`, one tool per skill of the card,
  behind the `GuardedToolExecutor` - is a second step; it shares the client
  but not the actor. Open there: a tool call is one request and one
  result, an A2A task may take minutes and ask back.
- **Caelumora as an A2A server.** Our agents publishing cards and taking
  tasks: a frontend like the REST server, with authentication and a
  security context for the caller. Largest and least urgent.
- **Unreachable agents.** As for MCP servers (issues
  [32](32-unavailable-tools-invisible-to-model.md),
  [126](126-mcp-tools-discovered-once.md)): a card that cannot be read must
  leave a trace, and a card that changes should be followed.
- **Protocol version.** A2A v1.0 only; v0.3 peers are not worth a second
  code path.

### An agent to test against

- **Hermes Agent** (Nous Research) speaks A2A v1.0 natively since v0.20.0,
  in both directions, runs locally and is open source - the first
  counterpart, also for the inbound direction later.
- **Claude Code** has no A2A of its own; only community wrappers around the
  Agent SDK exist, and then the wrapper is what is tested.
- **Claude** as such is a model, not an agent: it belongs behind the
  `LLMClient` component, not behind A2A.
- For conformance, an agent built on the official `a2a-sdk` (or Google ADK)
  is the reference.

## Implementation plan

Written against the code as of 2026-10-02; line numbers will drift.

### A2A in one paragraph

A2A v1.0 is request/response: every `SendMessage` (or `SendStreamingMessage`,
SSE) returns a `Message` or a `Task`. A task has an `id`, a `contextId`
(the conversation; we give it back so that the remote agent keeps the
thread) and a state: `SUBMITTED`, `WORKING`, `INPUT_REQUIRED`,
`AUTH_REQUIRED`, `COMPLETED`, `FAILED`, `CANCELED`, `REJECTED`. A stream
yields the task, then `statusUpdate` and `artifactUpdate` events. Other
operations: `GetTask`, `CancelTask`, `SubscribeToTask` (re-attach to a
running task), `ListTasks`. The card is at `/.well-known/agent-card.json`
and lists `supportedInterfaces` (url, `protocolBinding`,
`protocolVersion`), `capabilities.streaming`, `securitySchemes` and
`skills`. Clients send `A2A-Version`. There is no "for your information"
message: whatever we send, the remote agent answers. Hence the flow below
sends when asked and catches the remote agent up on what was said
meanwhile.

### What is built

**Model (`caelumora-model`)**

- `PersonalityKind { Internal, External }` (default `Internal`) and
  `Personality.kind: PersonalityKind` with `#[serde(default)]`, so that
  seeds and stored personalities need no change. Seven places build a
  `Personality` literally (`store_impl/suite.rs`, `runtime/template.rs`,
  `channel.rs`, `channel_members_prompt_provider.rs`, `controller/agent.rs`,
  `controller/mod.rs`, `view/mod.rs`) and gain the field.
- `Personality.roles: Option<Vec<String>>` and
  `Personality::limiting_roles() -> Option<&[String]>`: the given roles, or
  for an external personality that names none, none (public only); `None`
  for an internal one that names none.
- `ExternalAgentChannelState`, stored under a key of its own
  (`StateStore::get/put_external_agent_channel_state(agent, channel)`,
  memory and JSON stores, `store_impl/suite.rs`), not under
  `AgentChannelState`'s - the JSON store would fail to read a different
  type from the same key, and the fields of a turn mean nothing here
  (issue 177). It holds:
  - `history: MessageHistory` - what we sent it and what it answered,
    which is all we know of it (issue 06 for an external agent);
  - `conversation: Option<ConversationId>` - the remote conversation (A2A
    `contextId`), one per channel;
  - `open_tasks: Vec<OpenTask { task: TaskId, since: DateTime, state }>` -
    tasks not yet terminal, to be resumed after a restart;
  - `caught_up_to: Option<Digest>` - the last entry of the channel history
    the remote agent was given, so that a request after a restart sends
    what was missed, read from the channel's own history through the views
    (a channel may always read itself).
- `ActorRuntime::load/store_external_channel_state` beside the agent ones,
  both requiring `actor.as_agent_id()`.

**Facet `ExternalAgent` (`caelumora-runtime/src/external.rs`)**, next to
the actor that uses it, protocol-neutral - the actor knows only this:

```rust
#[async_trait]
pub trait ExternalAgent: Send + Sync + Debug {
    /// Hands `message` to the remote agent within `conversation` (`None`
    /// opens one) and streams what comes back until the task is terminal
    /// or waits for input. Dropping the stream abandons the request, not
    /// the task.
    async fn send(&self, security: &SecurityContext, conversation: Option<&ConversationId>,
                  message: OutboundMessage) -> ExternalResult<BoxStream<'static, ExternalResult<ExternalEvent>>>;
    /// Re-attaches to a task left open - after a restart, or one that
    /// asked for input and was answered.
    async fn resume(&self, security: &SecurityContext, task: &TaskId)
        -> ExternalResult<BoxStream<'static, ExternalResult<ExternalEvent>>>;
    async fn cancel(&self, security: &SecurityContext, task: &TaskId) -> ExternalResult<()>;
    /// How its answers are classified, if configured (else the channel's).
    fn classification(&self) -> Option<Classification>;
}

pub struct OutboundMessage { pub parts: Vec<OutboundPart>, pub answers: Option<TaskId> }
pub enum OutboundPart { Text { sender: String, text: String } }
pub enum ExternalEvent {
    Conversation(ConversationId), Started(TaskId),
    Partial(String), Message(String),          // a chunk; a whole message or artifact
    InputRequired(String),                     // its question; the task stays open
    Completed, Canceled, Failed(String), Rejected(String), AuthRequired,
}
```

`ConversationId` and `TaskId` are opaque strings of the protocol. The
`SecurityContext` is passed as `mcp_tool` passes it to
`SecretManager::resolve` on every call. `ExternalError` is a `thiserror`
enum (`Unreachable`, `Rejected`, `Protocol`, `Secret`) whose messages name
the component, never the URL with credentials (`LLMError::redact_endpoint`
is the pattern).

**Component `a2a_agent` (`caelumora-runtime/src/external_impl/a2a_agent.rs`)**,
a new directory because the facet is its purpose (COMPONENTS.md, *Writing
a component*); `external_impl::register_all` registers the kind and
`registry.single::<dyn ExternalAgent>(A2A_AGENT_KIND)`.

```rust
pub const A2A_AGENT_KIND: &str = "a2a_agent";
#[derive(Deserialize)]
struct A2aAgentConfig {
    url: String,                                  // where the card is, or the JSON-RPC endpoint
    #[serde(default)] auth: Option<RemoteAuthConfig>,
    #[serde(default = "default_timeout_secs")] timeout_secs: u64,   // per request, not per task
    #[serde(default)] classification: Option<Classification>,
}
pub struct A2aAgentComponent { id, config, card: ArcSwap<Option<AgentCard>>, http: reqwest::Client, secret_manager }
```

- Built with `register_async`, like `mcp_tool`: parse the params
  (`ComponentError::InvalidConfig`), fetch the card under a timeout,
  choose the interface with `protocolBinding` JSON-RPC and
  `protocolVersion` 1.x - any other version is refused, no v0.3 path. A
  card that cannot be read is a `warn`, the component is built and every
  `send` fails as `Unreachable` naming the component (the analogue of
  issue 32; a refresh is issue 126's).
- `send`: `SendStreamingMessage` with SSE if `capabilities.streaming`,
  else `SendMessage` with `returnImmediately: false`; headers
  `A2A-Version` and the resolved bearer. JSON-RPC and SSE over `reqwest`
  (already a dependency, `stream` feature on); no A2A crate exists for
  Rust that is worth depending on, the few types needed are written as
  `serde` structs in the file. Stream events map one to one onto
  `ExternalEvent`.
- `resume` is `SubscribeToTask`, `cancel` is `CancelTask`;
  `TaskNotCancelableError` is not an error for us.
- **Auth shared with `mcp_tool`:** `McpAuthConfig { Bearer { secret } }`
  and `resolve_auth_header` move to `crate::remote` as `RemoteAuthConfig`
  and are used by both (AGENTS.md: a second implementation of something
  that exists is aligned, not copied).
- Registered as the default kind of the `ExternalAgent` single, which
  `ComponentRegistry::verify` demands; an external personality that wires
  no component gets the default built with empty params, which fails as
  `InvalidConfig` naming `url` - the error the maintainer asked for.

**`Member::ExternalAgent` and the actor's construction
(`caelumora-runtime/src/channel.rs`)**

- `Member::of` loads the personality for `Participant::Agent`
  (`entity_store().get_personality`, as system; a personality that is gone
  is an error, not `None` - the agent exists and is broken) and yields
  `Member::Agent` or `Member::ExternalAgent(Arc<AgentController>)`.
- `create_actor` builds `ExternalAgentActor::new(&channel, agent,
  actor_runtime)` for it; `seed_transcript` runs for `Member::Agent` only.

**`ExternalAgentActor` (`caelumora-runtime/src/actor_impl/external_agent.rs`)**

```rust
pub struct ExternalAgentActor {
    agent_id: AgentId, agent_controller: Arc<AgentController>, actor_runtime: ActorRuntime,
    security_context: SecurityContext,            // owner's, .next(ActorId::Agent(id)), as AgentActor
    remote: ComponentImpl<dyn ExternalAgent>,
    state: Mutex<ExternalAgentChannelState>,
    processing: ProcessingStateHolder,            // Idle, or Planning while a task runs (see below)
    current: std::sync::Mutex<CancellationToken>, // the task under way
    ended: CancellationToken,
}
impl ExternalAgentActor {
    pub async fn new(channel: &Channel, agent: Arc<AgentController>, actor_runtime: ActorRuntime) -> Result<Self>;
}
impl Actor for ExternalAgentActor {
    fn kind(&self) -> String { "external_agent".into() }
    fn event_filter(&self) -> fn(&Envelope) -> bool;   // ResponseRequested | FullResponseGenerated
    // state(), cancel(), stop(); wakeup() is a no-op - it never sleeps
}
```

`new` loads the personality, builds the container as `AgentActor::new`
does and asks `components.one::<dyn ExternalAgent>()`; loads the state; and
checks once that `remote.classification()` (or the channel's) may be
published into the channel (`AccessQuestions::may_publish`), so that a
component classified above its channel fails at construction rather than
having every answer dropped silently by `ChannelLoop::admits`.

### The flow

1. **Hearing.** `FullResponseGenerated` of others reaches the actor as
   the moderator re-wraps it (user-role messages, tool calls stripped). The
   actor records nothing of it - the channel history is the record; it
   only notes that there is something new.
2. **Asked.** On `ResponseRequested`: publish `ResponseStarted`; read the
   channel history since `caught_up_to` through the views; render each
   message as a text part with its sender's name (the rendering the
   `ContextResolver` uses for sender names, issue 135, reused rather than
   a second format); `remote.send(conversation, message)`; set
   `processing` to working.
3. **Streaming.** `Conversation(id)` is stored; `Started(task)` is added
   to `open_tasks`; `Partial` is published as `PartialMessageGenerated`;
   `Message` chunks are collected; `InputRequired(q)` makes the question
   the answer and leaves the task open.
4. **Done.** On a terminal event or `InputRequired`: build the `Message`
   (`role: Assistant`, `sender_id: ActorId::Agent(id)`, `sender_name`,
   `classification: remote.classification()` or the channel's,
   `Message::new`), append what was sent and what came to `history`, move
   `caught_up_to`, drop the task from `open_tasks` if terminal, store the
   state, `announce_channel_state_changed`, publish
   `FullResponseGenerated(vec![message])`. `ChannelLoop::admits` asks
   `Publish` in the actor's context, `resolve_mentions` handles its
   `@`-mentions, `append_history` records it. `Failed`/`Rejected`/an
   `ExternalError` publish `TurnFailed { message }` and store the state;
   the actor goes on (an error ends the response, not the actor).
5. **Open tasks.** A task that asked for input is continued by the next
   request (`OutboundMessage.answers = Some(task)`). Events on an open task
   outside a request - a long task finishing, a remote agent with more to
   say - are heard through the stream kept open (or `resume`) and published
   as in step 4, preceded by `WakeupRequested` as `UserActor::send_message`
   does, so that sleeping agents hear it. That is the remote agent speaking
   on its own.
6. **Cancel.** `cancel()` cancels `current`: the stream is dropped,
   `remote.cancel(task)` is attempted, and what arrived so far is published
   as `FullResponseGenerated`, so that observers learn generation stopped
   (as `AgentActor`'s `Cancelling` does, issue 148). `ChannelLoop::cancel`
   sets `quiet` and calls this.
7. **Stop.** `stop()` cancels `ended`; the run loop cancels the current
   task best-effort, stores the state and returns.
8. **Restart.** `new` loads the state; for every open task the run loop
   calls `resume` first thing: terminal meanwhile - publish as step 4;
   still working - keep listening; input required - publish the question.
   `ResponseRequested` arriving while this runs is queued, as every actor
   queues during a turn.

### Clearance at the door

- `RuntimeManager::limiting_roles_of(actor) -> Result<Option<Vec<String>>>`:
  for an agent, `limiting_roles()` of its personality; `None` for anyone
  else. Loaded as system, like `roles_of`.
- `RuntimeManager::require_cleared` and `session.rs::require_cleared` ask
  `JoinChannel` with the owner's roles as today and, when
  `limiting_roles_of` gives some, a second time with those. Both must allow;
  `ChannelError::NotCleared`/`SessionError::NotCleared` as today. Raising a
  channel asks for everyone already (channel.rs `reclassify`).

### Steps

Each step compiles, is tested, and leaves `main` working.

1. **Model.** `PersonalityKind`, `Personality.kind`,
   `Personality.roles`, `ExternalAgentChannelState` with its store
   methods (memory, JSON, suite) and `ActorRuntime` accessors. No
   behaviour changes.
2. **Facet and a scripted stand-in.** `external.rs` with the trait and
   events; `external_impl/` with `register_all` and the single; a
   `scripted_external_agent` test kind that answers from a script, built
   like the channel tests' `Scripted` model.
3. **Actor, request/response.** `Member::ExternalAgent`, `create_actor`,
   `seed_transcript` narrowed, `ExternalAgentActor` with steps 1-4, 6 and
   7 of the flow. Tests built like `channel.rs::with_an_agent`: a user
   speaks, the external agent is asked, what it was given and what it
   answered are in channel and agent history; cancel and stop; a failing
   remote yields `TurnFailed` and the actor goes on; a personality of kind
   external without the component fails to build with an error naming
   `url`.
4. **Clearance.** `limiting_roles_of`, the second `JoinChannel` at both
   sites. Tests: an agent whose personality is limited below the channel is
   refused on `add_participant` and in a template's channel, internal and
   external alike; an external one naming no roles joins only a public
   channel; an owner cleared for less is refused too.
   CONFIDENTIALITY.md: clearances, destinations, the privacy limit.
5. **Open tasks.** `InputRequired`, continuing a task, unprompted events
   with `WakeupRequested`, `resume` after a restart. Tests with the
   scripted stand-in: a task that asks back and is answered; a task that
   finishes after a restart of the channel.
6. **`a2a_agent`.** Card discovery, JSON-RPC, SSE, `A2A-Version`, auth
   shared with `mcp_tool` (`crate::remote`), classification. Tests
   against an `axum` stub speaking A2A v1.0 (dev-dependency exists, as the
   `mcp_tool` tests do), covering streaming and blocking, input-required,
   cancel, an unreadable card, a v0.3-only card refused. Then by hand
   against Hermes Agent, with what was found recorded here.
7. **Documentation and seed.** CHANNELS.md (the fourth actor, `Member`),
   COMPONENTS.md (facet table, built-in table, `external_impl/`),
   STORAGE.md (the new state), ARCHITECTURE.md and README,
   `examples/seed.yaml` with an external personality; INDEX and this
   file's status.

### Things to watch

- **Nothing of the turn.** No `Turn`, no `TurnState`, no `MAX_MODEL_CALLS`:
  a remote task may run as long as it runs. The actor's only timeouts are
  per request, in the component.
- **`ActorState`.** Its variants are a turn's (`Planning`,
  `ExecutingTools`, ...). The external actor reports `Idle` or `Planning`
  ("the remote agent is working") rather than adding a variant the web UI
  would have to learn; the moderator only asks whether it is `Sleeping`,
  which it never is. If this reads wrong in the UI, a variant is a
  follow-up.
- **What the views show.** `MemberView::stored_state` reads
  `AgentChannelState` for every `Participant::Agent` and gets `Default`
  for an external agent: no notes, no context, no pending approvals - true
  and harmless. Showing what the external agent was given (its
  `history`) in the context panel is a follow-up; the data is kept for
  it.
- **The web UI** derives `ParticipantKind` from the `ActorId` (`agent`),
  and `ChannelMember.kind` carries the actor's `kind()` string
  (`external_agent`); check that the UI tolerates a string it does not
  know. The REST API does not change.
- **Mentions by the remote agent.** `resolve_mentions` resolves them, but
  `mentioned_responders` only routes mentions of `User` senders, and
  `user_driven` asks no one after an agent speaks. An external agent that
  addresses another agent is therefore not answered under those policies -
  the same as for an internal agent today; whether that should change is
  a question of the moderator, not of this issue.
- **Rendering for the remote side.** What we send carries sender names in
  the text; it is the remote agent's "user" message. Do not send
  reasoning, tool calls or anything `published()` strips - the moderator's
  re-wrapping already does, and the history read through the views is the
  published form.
- **Classification of answers** is the component's or the channel's; a
  component classified above its channel fails at construction, not per
  answer. A2A `metadata` on messages could carry a classification later
  (issue 139's sibling) - not now.
- **Secrets and logs.** The bearer is resolved per request and never
  stored on the actor; errors name the component, not the URL; what was
  sent or received is logged at `trace!` only.
- **No `unwrap`**, and an error in a response ends the response
  (`TurnFailed`), never `run`.

## Done when

- A personality of kind external, with an `a2a_agent` component, yields an
  agent that can be added to a channel like any other, answers when
  addressed, speaks when its remote agent does, and survives a restart with
  its open tasks.
- An external agent joins only channels its owner and its component are
  cleared for, and the privacy limit is documented in CONFIDENTIALITY.md.
- Tested against Hermes Agent.

## History

- **2026-10-02** — Issue created at the maintainer's request.
- **2026-10-02** — Architecture decided with the maintainer. An
  `AgentActor` with the remote agent as `LLMClient` was proposed and
  rejected: the facet cannot speak unprompted, and the turn, tools and
  approvals would be ballast. Decided: an `ExternalAgentActor` of its own,
  `ActorId::Agent` kept, `Personality.kind` says which actor an agent gets
  (a field, not derived from the components), `Member::ExternalAgent`
  derived from it, and the protocol as a component behind an
  `ExternalAgent` facet, `a2a_agent` first.
- **2026-10-02** — Confidentiality decided with the maintainer: clearance at
  join (`JoinChannel` with the component's `roles` and the owner's, both
  must allow), then the remote agent is shown everything said in the
  channel. A per-message check and "the owner decides" were weighed and
  rejected. Opened [180](180-outbound-flow-unchecked.md) for MCP arguments,
  which leave the system unchecked by the same measure.
- **2026-10-02** — Implementation plan written, at the maintainer's
  request, after reading the actor, channel, component, view and
  moderator code. Two findings shaped it: A2A is request/response, so the
  remote agent is caught up on what was said when it is asked rather than
  sent every message; and the channel state is typed `AgentChannelState`
  throughout, so the external agent gets a state and a store key of its
  own.
- **2026-10-02** — Step 1 done: `PersonalityKind` and `Personality.kind`
  (`internal` unless named), `ComponentDescriptor.roles`, the model module
  `external_agent` with `ExternalAgentChannelState`, `ConversationId`,
  `TaskId`, `OpenTask`, kept by both stores under a key of their own and
  deleted with the channel, and `ActorRuntime::load/store_external_channel_state`.
  One deviation from the plan: `caught_up_to` is the `MessageId` of the
  last message given, not a digest - the id stays the same when the chain
  around it changes (a branch, issue 08), a digest does not.
- **2026-10-02** — Where the roles live, decided again with the maintainer:
  not on the `ComponentDescriptor`, where step 1 had put them - a field of
  every component that means something for one kind only - but on the
  personality, as a user has roles. They limit the owner's clearance and
  never replace it; an external personality that names none sees only what
  is `public`, an internal one is not limited. `ComponentDescriptor.roles`
  is removed again, `Personality.roles` and `limiting_roles` take its
  place. Opened [181](181-where-roles-decide.md): where roles enter a
  permission check at all has never been reviewed.
- **2026-10-02** — Step 2 done: the facet `ExternalAgent` in
  `caelumora-runtime/src/external.rs` with `send`, `resume`, `cancel` and
  `classification`; `OutboundMessage` (what was `said`, by whom, and the
  task it `answers`), `ExternalEvent`, `ExternalStream`, and
  `ExternalAgentError`, shown to users like `LLMError` and never naming
  the secret behind a failed credential. `ScriptedExternalAgent` is the
  stand-in for the tests of step 3 on. `external_impl::register_all`
  registers `ExternalAgent` as a single. Two deviations from the plan:
  - `ExternalEvent` has one `Failed(reason)` for a task the remote agent
    failed or refused, and no `AuthRequired` - the actor treats them
    alike, and a protocol maps its own states onto them.
  - The single has no default, decided with the maintainer: a single may
    have none (`ComponentRegistry::single::<T>(None)`), and
    `ComponentRegistry::verify` is gone. A container asked for a single it
    does not provide and that has no default fails to build it, as one
    whose default kind is not registered as providing it fails the first
    time the default is needed. Rejected: `a2a_agent` as the default (A2A
    as the silent protocol), a kind that only fails, and no single at all
    (the actor counting its providers).
