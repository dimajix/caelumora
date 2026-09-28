# Caelumora Architecture

Caelumora is a multi-agent system designed for flexible, extensible, and scalable agentic workflows. It provides a robust framework for managing sessions, channels, and various types of actors (including humans and AI agents) that communicate through structured events.

## Top-Level Modules

The project is organized into several key modules:

*   **`caelumora-model`**: Contains the core data structures and domain entities (e.g., `Agent`, `Session`, `Message`, `ComponentConfig`). It is shared across both client and server.
*   **`caelumora-runtime`**: The heart of the system. Implements the execution logic for agents, sessions, and channels. It handles event routing, component management, and the agent cognitive cycle.
*   **`caelumora-rest-server`**: Provides a REST API for interacting with Caelumora, managing sessions, and participating in conversations.
*   **`caelumora-client-api`**: Defines the traits and interfaces for client-side interactions.
*   **`caelumora-local-client`**: Implementation of a client that runs in the same process as the runtime (useful for testing and TUI).
*   **`caelumora-tui`**: A terminal user interface for Caelumora.

## Component Architecture

Caelumora uses a pluggable component-based architecture for defining agent behavior.

*   **`ComponentConfig`**: A declarative definition of components and their dependencies.
*   **`ComponentNodeConfig`**: Configuration for an individual component, including its kind, parameters, and dependencies.
*   **`ComponentRegistry`**: A central registry where component factories are registered by their kind.
*   **`ComponentContainer`**: Responsible for instantiating and managing the lifecycle of components based on the `ComponentConfig`.
*   **Facets**: A component offers one or more facets - traits it implements to take part in the agent loop: `ToolProvider` (tools), `CommandProvider` (user commands), `PromptProvider` (prompt parts), `ContextProvider` (what the model is given), and `PerceiveInProcessor` / `EmitOutProcessor` (the message stages). Each facet trait lives next to the code that uses it - `crate::tool`, `crate::command`, `crate::context`, `crate::message` - not among the implementations (issue 53).

The built-in components live in `caelumora-runtime/src/component_impl/`, **one file per component kind**, named after the kind (`calculator_tool.rs`, `notes.rs`, `state_context_provider.rs`, ...), each with its kind constant, its `register` function and its tests; `component_impl::register_all` registers them all (issue 52). They are sorted by component, not by facet, because one component may offer several facets - `create_plan_tool` and `notes` provide tools and a prompt part. A new component gets a file of its own and a line in `register_all`; `component_impl` holds components only.

### Tool Components

Agents gain capabilities through components implementing `ToolProvider` (one component can expose multiple `Tool`s). Built-in providers include `echo_tool`, `calculator_tool`, and `stop_tool`. An `mcp_tool` component exposes the tools of a remote MCP (Model Context Protocol) server, reached over the Streamable HTTP transport:

```json
{ "id": "github-mcp", "kind": "mcp_tool", "params": { "url": "https://mcp.example.com/mcp", "auth": { "type": "bearer", "secret": "GITHUB_MCP_TOKEN" } } }
```

Multiple `mcp_tool` instances (one per server) can coexist; each server's tools are exposed namespaced as `"{component_id}__{remote_tool_name}"` to avoid name collisions. The tool list is discovered once when the component is built and cached; a server that is unreachable at that point does not fail the whole agent — it simply starts with no tools available. `auth.secret` names a secret resolved through the secret-provider chain (see below), never a literal credential.

An agent's `ToolExecutor` is built from the `ToolProvider`s of its components, with their component ids. Two components providing a tool of the same name are a configuration error that keeps the agent from starting, naming both components — resolved by order, one of them would be silently unreachable (issue 37). User commands follow the same rule.

A `Tool` declares its `ToolConfidentiality` (issues 54, 61): the confidentiality labels of its calls - how far the call itself may travel, and how far its content, the arguments and the result. `PUBLIC` is the default, `CALL_ONLY` lets others see that the tool was called but not with what, `HIDDEN` keeps the whole call with the agent. The agent stamps every call with the labels its tool declares at that moment, and the result takes them over from the call, so both are stored with what applied then. The agent keeps the full calls in its context; what it publishes is redacted in one place, `TurnContext::send_event`, by the labels its channel admits (see *Confidentiality*): a call whose call label is not admitted is left out, one whose content is not has its arguments set to `null` and its result emptied, and a message with nothing left is not sent. The channel history is built from these events, so history, REST, the event stream, `read_history` and the moderator only ever see the published form; the context panel (`ReadAgentContext`) shows the agent's own, full view. Approval requests are not redacted - the approvers see the arguments of a call they are asked to decide. Since the answer text is always published, the model is told what it must not quote (issue 66): a successful result whose content label the turn may not publish reaches the model wrapped as `{"result": ..., "note": "Only you see this result ..."}` (`tool::successful_result`), and the private notes' prompt part and tool description say not to reveal them.

## Secret Management

Component configuration (such as an `mcp_tool`'s `auth.secret`) may need to reference credentials without embedding them directly in persisted config. `SecretProvider` (`caelumora-model`) is a pluggable source of secrets; a `SecretManager` holds an ordered list of providers and tries them in turn, returning the first match. Built-in providers are `FileSecretProvider` (a flat JSON key/value file, re-read on every lookup so secrets can rotate without a restart) and `EnvSecretProvider` (process environment variables). `AppConfig`'s `secrets` section configures the provider chain; the default, if unspecified, is `[File("local/secrets.json"), Env]`. Every `SecretManager::resolve` call is gated through the `SecurityManager` (`ActionType::ReadSecret`) before any provider is consulted, consistent with how `EntityStore`/`StateStore` gate access on a `SecurityContext`.

## User Providers

Where users come from and how their credentials are verified is pluggable in the same way as secrets. A `UserProvider` (`caelumora-model`) is one source of users; a `UserManager` holds an ordered chain of them, configured in `AppConfig`'s `users` section. Looking a user up (`find_user`, `get_user`) and verifying a credential (`authenticate`) are separate operations, and verification never hands out the stored credential, so backends that verify by binding (LDAP) fit the same trait as backends that compare a hash. How a provider assigns `UserId`s is its own business; the contract is only that the same user keeps the same id, across restarts included.

The chain differs from the secret chain in how it fails: `Ok(None)` passes the question to the next provider, but a rejected credential or a failing provider ends it. A broken user file must deny, never fall through, and an unknown user is reported exactly like a wrong password. There is no default provider. The only built-in one so far, `accept_any`, is a development stand-in that accepts any username and password and keeps users in the `EntityStore`; it must be enabled explicitly and warns at startup (issues 10, 25).

## Agent Cognitive Cycle

The agent cognitive cycle (implemented within `AgentActor`) defines how an agent processes information and generates responses. It typically follows these stages:

1.  **Perceive**: Incoming messages are processed by "perceive" components (e.g., adding to history, updating context).
2.  **Plan**: The agent decides what to do next. This may involve interacting with an LLM, planning tool calls, or generating internal thoughts.
3.  **Execute**: If the plan involves tool calls, the agent checks if any require manual approval. If so, the cycle is suspended (see *Human-in-the-Loop Approvals*). Otherwise, tools are executed and results are fed back into the loop.
4.  **Emit**: The final response messages are generated and emitted to the channel.

The loop is stateful, maintaining an `AgentState` which includes `MessageHistory` and `MessageContext`.

### What an agent saw

For every message an agent produced, it can be told what its model saw when it wrote it (issue 06). Its `AgentChannelState` keeps the `MessageContext` - the messages the model is given - as a chain: every entry has a `Digest` (SHA-256, `caelumora_model::digest`) over its message and the digest before it, so one digest names the whole context up to that point by content. What goes into a message's digest is fixed in one place, `digest::message_chain`, behind a version tag: id, role, sender, content, reasoning, tool calls and results, and whether it is internal - not timestamps, usage statistics, resolved mentions or metadata. JSON values such as tool arguments are hashed with sorted keys, so the digest does not depend on how a value was built or on `serde_json` features.

The chain itself is one generic type, `caelumora_model::chain::Chain<E>` (issue 48): entries that are only appended to, each storing the digest its type computed over its content and the digest before it, with lookup of the prefix an earlier digest names. `MessageContext` and `MessageHistory` are chains of messages and share the message-specific part - the `message_chain` digest, and `prev_id`, which each chain sets to its own last message, so the same message has different predecessors in different chains. A channel's notes ([17](issues/17-information-sharing-within-session.md)) will be a chain too, and branching ([08](issues/08-retry-and-edit-past-answers.md)) will turn the one type into a tree.

At the end of a turn, `TurnContext::record_turn` appends the agent's messages to its context and the non-internal ones to its history, each with a `ContextRef`: the digest of the context before the message, and the turn's prompt parts. `AgentChannelState::seen_context_of(message)` resolves a reference to the messages and prompt parts the model saw, in the order it was given them; the channel's history identifies the agent by the message's sender.

#### Prompt parts

What the model is given besides the conversation - the system prompt, and later notes ([17](issues/17-information-sharing-within-session.md)) - is a *prompt part* (issue 47). The `ContextProvider` decides where parts go, the `TurnContext` records what it was handed:
*   A `PromptProvider` contributes a part with a `PromptPosition`: `Start` (the system prompt) or `BeforeTurnInput`. `StateContextProvider` joins the parts of the providers it depends on per position, in the order the dependencies are declared; `static_system_prompt_provider` takes its position from the parameter `position` (`start` by default, or `before_turn_input`).
*   `Start` goes before everything, as a system message. `BeforeTurnInput` goes transiently before the newest message of the turn - at the end if the agent wrote that itself, in a turn without new input - as a user message: many models cache the prompt up to the first change, and some merge every system message into the system prompt at the front. Information that changes often therefore does not belong in the system prompt, and a part is never stored in the context, which stays a stable prefix. `ContextProvider`s run once, at the start of a turn, so a part stays in place across the tool loop.
*   The provider hands the turn a `Vec<ContextItem>`: stored context entries with their digests, and prompt parts with the role they are given as. `TurnContext::set_context` keeps every part's text in `AgentChannelState::prompts`, once per digest, and records a `PromptRef` for it - the text's digest, its role, and the digest of the context entry it followed (`None`: before everything). Placing and recording are one step, so what the model was given and what is recorded as given cannot diverge. `resolve` inserts the parts after their entries again.

#### Notes
An agent notes what it wants to remember in a channel with the `notes` component (issue 17): its tools `note` and `revoke_note` change the agent's own notes, and its prompt part gives the model the notes of every agent in the channel. Notes belong to the agent that wrote them and are published in the channel (principles in [19](issues/19-memory-tiers.md)):
*   Each agent keeps its notes per channel in `AgentChannelState::notes`, a `NoteChain` on the chain type: entries "note" or "revoke N", each with the message it follows and the context digest; what is noted is the chain read from start to end. Numbers count up per agent and are never given again, so that a model can repeat them to revoke. Only the author revokes.
*   The logic is the wrapper `agent_notes::AgentNotes` (numbering, noting, revoking; `NoteKind::Public` or `Private`), which the components use; `TurnContext` knows nothing of notes (issue 65). A change is staged as an `AgentChannelChange` (`TurnContext::stage_channel_change`) and applied by `record_turn` when the turn ends - generically, through `AgentChannelState::apply` - after the turn's last message in the history; a turn that fails leaves none. The agent then stores its channel state and announces `GlobalEvent::AgentChannelStateChanged` (with the channel's attributes, without content) before `FullResponseGenerated` ends the turn, so that whoever reacts to the end of the turn reads the state it left.
*   Others read published notes from the agents' states in the `StateStore` (`ChannelLoop::published_notes`), so only notes of finished turns are ever seen. The notes prompt part is a `BeforeTurnInput` part, empty when nothing is noted.
*   The channel snapshot carries `notes` per agent; the web UI shows them in a "Notes" tab next to the members and fetches them again (`GET …/channels/{cid}/notes`) when the global stream announces `AgentChannelStateChanged` for the channel.
*   **Other channels.** Components read through a `SessionReader`, given to them when they are built (`ComponentContext::session_reader`; `session_reader::detached()` outside of a channel), which reaches the other channels of the session through `ActorRuntime::session()` - every `ChannelRuntime` of a session shares a weak handle to its `SessionLoop`, set by `SessionLoop::attach`. Another channel is read only if the reading channel may read it (`ChannelLoop::may_read`, `ActionType::ReadChannel`), which depends on the two channels' confidentiality levels, not on who takes part in them (see *Confidentiality*): an agent passes on what it reads to its channel. A channel that does not exist and one that may not be read are the same error, so channels cannot be probed. The tools are `read_notes(channel?)` (in `notes`; without a channel, all readable channels) and `read_history(channel, last?, query?)` (component `channel_history`).
*   **Private notes** (issue 51). The `private_notes` component gives an agent a second chain per channel, `AgentChannelState::private_notes`, with its own tools `note_private` and `revoke_private_note` and its own numbering, so that gaps in the public numbers do not reveal private notes. Changes are staged and taken over with the turn like public ones (`AgentChannelChange::PrivateNote`). Its `BeforeTurnInput` part shows the agent its private notes of this channel and - through `SessionReader::private_notes` - of every other channel of the session that this channel may read (`ChannelLoop::may_read`, the same rule as for reading another channel): a note carries the confidentiality of the channel it was written in, checked against that channel's current level when it is read. Both tools are `ToolConfidentiality::HIDDEN`, so the calls never reach the channel; "private" means private from other agents and other users, not from the agent's owner, who sees them in the context panel, which is theirs alone (issue 54).
*   **Recording what was read.** Every read is a `KnowledgeRef` - the channel, and `Notes { agent, digest }` or `History { digest }` - recorded by `TurnContext::record_knowledge` with the number of messages the turn had produced; `record_turn` puts it into the `ContextRef` of every message produced after it (`ContextRef::knowledge`). A `PromptProvider` returns a `Prompt` with its text and the knowledge it contains, so the notes prompt part records its reads as well. The message context over REST lists the knowledge with channel and agent names; the context panel shows it as "Knowledge read".

Asking for it is an operation of the actor that produced the message, not of the channel: the same message is in every participant's history, but only one of them produced it. `Actor::message_context(caller, message_id)` answers with context and prompt parts; only `AgentActor` has one, every other actor answers `NotRecorded`. The agent asks the `SecurityManager` with `ActionType::ReadAgentContext` and its own attributes - under the ownership policy only the agent's owner may, since a context holds more than the conversation: the system prompt, internal steps, tool results - and reads its state from the `StateStore`, where it is written after every step (the running loop holds the in-memory state). REST exposes it as `GET /sessions/{id}/channels/{cid}/actors/{aid}/messages/{mid}/context`, answering `403 context_forbidden`, `404 context_not_recorded` (not produced by that actor) or `404 context_unavailable` (recorded, but no longer resolvable).

So that a client offers this only where it works, every member of a channel response carries `can_read_context`, decided for the requesting user by `Actor::may_read_context` - the same policy question. The web UI shows an eye icon on stored messages of agents the user may read, which opens a panel with the context's messages and prompt parts where they were given, a leading system prompt on top (issue 41); `MessageView` renders a message the same way in the chat and in that panel, marking internal messages and tool results, which the chat itself does not show.

A digest is a record of what was sent to the model, not a value to derive again: it is stored with its entry and never recomputed. Were the scheme to change, recomputing would silently change every digest the history refers to; stored, old entries keep their digests and only new ones get the new scheme's.

This relies on the context only ever growing: an earlier digest names a prefix of it. Compressing the context ([16](issues/16-context-compression.md)) produces a new context that came from the old one but does not continue it. Entries written before keep referring to the old context, which therefore has to remain resolvable, and only new entries refer to the new one; the link from the new context to the one it came from is a relation of its own, not the predecessor link of the chain, and - since it is not sent to the model - not part of the digest.

## Session, Channel, and Actor Design

Caelumora uses an actor-based model for communication within sessions.

### Session and SessionLoop
A **Session** represents a high-level container for a specific interaction context. The `SessionLoop` manages multiple channels within that session.

Channels can be created in and deleted from a running session (`SessionLoop::create_channel` and `delete_channel`, `POST /sessions/{id}/channels` and `DELETE /sessions/{id}/channels/{cid}`). Channels are part of the session document (`Session.channels`), so creating or deleting one changes the session. Like renaming, it is stored first through `SessionController::add_channel` or `remove_channel`, which checks `UpdateSession` so that only the owner may do it; only then is the running session changed and the change announced (`ChannelCreated`, `ChannelDeleted`). `SessionController` holds a lock while it changes and stores the session, so that concurrent changes cannot overwrite each other.
*   **Building a channel.** Every channel is built by `Channel::build`, from a template (`ChannelTemplate::instantiate`) or on request. It adds an approver and makes every user and agent in the channel a contributor. The participants of a requested channel come from the session's participants, the creator always among them, and its moderator defaults to `round_robin` and its confidentiality level to `session`, as in a template. A request that cannot be met - no name, a name the session already has, an unknown moderator, someone who is not a participant of the session, the level `private` - is a `SessionError`, refused before anything is stored.
*   **Running channels.** `SessionLoop::run` starts every channel in its `JoinSet`, those the session had and those created later, which it receives over a queue. Stopping the session aborts `run`, and with it every channel. Deleting a channel aborts that channel's task and deletes its state. A channel deleted while it waits to be started is never started.
*   Participants of a channel are still fixed once it is built; changing them is [46](issues/46-channel-participants.md).

### Channel and ChannelLoop
A **Channel** is a specific communication medium within a session (e.g., a chat room). The `ChannelLoop` is responsible for:
*   Managing the lifecycle of **Actors** participating in the channel.
*   Routing **Envelopes** (events) between actors.
*   Applying a **Moderator** to control message flow and determine which actor speaks next.

#### Acting in a channel

Whoever writes or approves does so in their own name: `ChannelLoop::user_actor(UserId)` returns *that user's* actor, and every envelope is built by the actor itself, so sender, name and `SecurityContext` follow from it. There is deliberately no way to reach "some" user actor. Someone who is not a participant of a channel has no actor in it and is refused with `403` — until participants can be added to a running channel ([09](issues/09-creating-channels.md)), that means the participants the channel was created with (issue 30).

Two things act on the channel as a whole and therefore ask the `SecurityManager` first, one decision for the whole channel rather than one per actor:

*   `ChannelLoop::cancel` (`ActionType::CancelChannel`) ends the current turn of every actor — the web UI's stop button. It is not a kill: a running tool or model call finishes, the agent reflects, emits `FullResponseGenerated` so observers stop waiting, and returns to a normal state with its history intact.
*   `ChannelLoop::wakeup` (`ActionType::WakeChannel`) is a *deliberate* wakeup, as `/wakeup` is (see *User commands*); `wakeup_actor` wakes a single actor under the same permission. Waking because something is on its way — a message being delivered, a mention, a `WakeupRequested` — goes through `wake_for_delivery` and asks nothing: it follows from an action that was already permitted, and a user allowed to write but not to wake would otherwise be unable to write.

A refusal is a `ChannelError::Forbidden`, which `caelumora_runtime::error::is_forbidden` recognises so that REST answers `403` rather than `500`; like a refused tool call, it says what was refused and not which rule refused it.

#### User commands

A line that starts with `/` addresses the system rather than the agents: `/help`, `/wakeup [agent]` ([28](issues/28-user-commands.md)). A command is not a message: it never enters the history and never reaches an agent's context. It is always run on behalf of a particular person, so it is run by that person's actor, `UserActor::execute_command(name, args)`, as messages and approvals are; REST reaches it the same way (`user_actor_of`, `403` for non-participants). Commands are for people, agents act through tools — only `UserActor` runs them. What a command does to the channel still goes through the channel's own, permission-checked operations (`ChannelLoop::wakeup`, `wakeup_actor`).

*   **Recognised by the frontend.** The client decides whether a line is a command and sends it to `POST .../commands` (`{name, args}`) instead of `POST .../messages`; the message API stays plain text. The rule is `caelumora_model::command::parse_input`, mirrored in the web UI's `command.ts`: a leading `/` starts a command whose name runs to the first whitespace, the rest of the line is handed to the command unparsed, and `//` sends a literal `/`. Only the lexical rule is repeated per frontend; what a command means, who may run it and what it answers is decided in one place.
*   **Commands are components.** A `CommandProvider` (`caelumora_runtime::command`) contributes commands, as a `ToolProvider` contributes tools, registered in the `ComponentRegistry` (`help_command`, `wakeup_command`). They come from two places: `AppConfig::commands`, the global set (default: `/help` and `/wakeup`; an empty list switches them off), and `SessionTemplate::commands`, for commands that belong to what a session is for, copied into the `Session` so a running session keeps its set. `RuntimeManager::new_command_executor` builds both once per session and hands the providers to a `CommandExecutor` shared by its channels — the counterpart of an agent's `ToolExecutor`, which likewise receives ready providers rather than a configuration. Two components providing the same name are a configuration error that keeps the session from starting, not something resolved by order.
*   **Seeing and running are separate permissions.** `ActionType::ShowCommand(name)` decides whether a command appears in `/help` and completion (`GET .../commands`); `ActionType::ExecuteCommand(name)` whether it runs. A command the user may not see is answered exactly like an unknown one, so hidden commands cannot be found by trying names. Whatever the command does is checked on its own as well — `/wakeup` asks for `WakeChannel`.
*   **Private or channel-wide.** A command answers the issuer alone (`/help`) or the whole channel (`/wakeup`). A channel-wide answer is announced by the issuer's `UserActor` as `ActorEvent::CommandExecuted`, which every moderator forwards to the users only and which, like `TurnFailed`, is shown but never stored. `UserActor::execute_command` returns the answer in both cases, because the channel never routes an envelope back to its sender; the REST response carries it only when it is private, since the web UI receives the announcement on its stream.

#### Channel and actor runtime

A `ChannelLoop` is built with a `ChannelRuntime`, not with the `RuntimeManager` itself. It carries what everything in the channel shares: the process-wide services of the `RuntimeManager`, the user commands of the session (a `CommandExecutor`, built once per session by `RuntimeManager::new_command_executor`) and a handle to the channel. The `ChannelRuntime` is the only source of `ActorRuntime`s, so every actor belongs to exactly one channel and reaches its commands (`ActorRuntime::command_executor`) and its channel (`ActorRuntime::channel`) through its runtime. The handle is a `Weak`, set once by `ChannelLoop::new` when the channel exists: the channel owns its actors, and a strong reference back would keep a stopped channel alive. `ComponentContainer::from_config` takes the registry and a `ComponentContext` — the security context and the `SecretManager` of the whole graph, to which it adds each component's dependencies — so building components needs no actor.

### Actor and ActorId
An **Actor** is any entity that can send and receive events in a channel.
*   **`Actor` trait**: Defines the interface for all actors, including an `async fn run` method.
*   **`ActorId`**: Uniquely identifies an actor, which can be a `User` or an `Agent`.
*   **`Envelope`**: The unit of communication, containing a timestamp, security context, event context, and the actual `ActorEvent`.

### Moderators
Moderators control how messages are routed and who is allowed to respond. A channel's moderator is named in its template (`ChannelTemplate.moderator`, default `round_robin`) and built by `RuntimeManager::new_moderator`.

All moderators route the same way: every message goes to every other participant, approvals, failed turns and announced commands go to whom they concern, and every agent a user `@`-mentioned is asked to respond. They differ only in whom they ask when a message mentions no agent. That difference is a `ResponderPolicy`, and `PolicyModerator` is everything else (`caelumora-runtime/src/moderator_impl/`), so a new moderator is a new policy:
*   **`broadcast`**: every agent responds.
*   **`round_robin`**: the agents take turns, in the order of their ids. A mention does not take a turn.
*   **`random`**: one agent, chosen at random.
*   **`mentions_only`**: nobody. Agents respond only when a user mentions them, and a message without a mention gets no answer.

The REST API returns each channel's `moderator`. The web UI shows it above the member list, and for `mentions_only` its input field says that agents answer only when mentioned. The TUI prints it when the chat starts.

#### `@`-Mentions
A user can address participants directly with `@handle` (e.g. `@sassy` for "Sassy"). A handle is the participant's name lowercased, with every run of other characters collapsed into `-` (`caelumora_model::mention`). The `ChannelLoop` resolves mentions once, as a message enters the channel, and stores the resolved `ActorId`s in `Message::mentions`; nothing downstream re-parses the text, so a later rename cannot change whom a past message addressed. Unknown handles stay plain text, and text in backticks is ignored.

A mention changes who is asked to respond, not who receives the message: every participant still gets it, but every moderator (via the shared `ModeratorActors::mentioned_responders`) sends `ResponseRequested` to exactly the mentioned agents instead of applying its own policy. Mentioned agents that are asleep are woken first. Only mentions written by a user are routed; mentions in agent responses are resolved and rendered but do not steer the moderator, so agents cannot address each other in an unbounded loop. The REST API lists each channel's `members` in no particular order, each with its handle (`null` for participants that cannot be mentioned, such as approvers); the web UI sorts them by name and uses them for the member list, `@` completion and highlighting.

## Controllers and State Management

Caelumora uses `Controller` types (e.g., `SessionController`, `AgentController`) for managing the live state of entities. 
*   **Live State**: Controllers use `ArcSwap` to provide atomic, lock-free access to the current state of an entity.
*   **Persistence**: Controllers are linked to the `EntityStore`. Any state modifications made through a controller (e.g., updating a name) are automatically propagated to the persistent store, ensuring data consistency across restarts.
*   **Eventing**: Controllers are responsible for publishing update events via the `EventBus` when state changes occur.

## Eventing and Observation

The `EventBus` (in `caelumora-runtime`) provides a centralized mechanism for observing changes across the system.

*   **`GlobalEvent`**: Represents any significant change in the system, such as creation, update, or deletion of sessions, channels, agents, or users.
*   **Subscriptions**: Clients can subscribe to specific `SubscriptionTopic`s (e.g., `All`, `Session(id)`, `Agent(id)`) and receive the relevant `GlobalEvent`s through an `EventSubscription`. Each subscription is a broadcast channel of its own: a subscriber that falls more than 128 events behind loses the oldest, and it is the subscriber that learns so (`Lagged`), not only the log (issue 38). `EventSubscription::recv` logs such a loss and goes on, for subscribers that cannot catch up; one that can takes the raw receiver with `into_receiver`.
*   **Publishing with attributes**: `publish(event, attributes)` takes, next to the `GlobalEvent`, the `SecurityAttributes` of the entity it is about — the session's, channel's, agent's, or for user events attributes owned by that user (issue 39). The bus wraps both internally; subscribers only ever receive the `GlobalEvent`, and the attributes never leave the process.
*   **Subscribing as someone**: `subscribe_as(context)` receives the events about entities `context` may read, asking the `SecurityManager` the same question the `EntityStore` asks for reading them (`GetSession`, `GetAgent`, `GetUser`). A deletion is decided by the attributes it was published with, since the entity is gone by then. These subscriptions are served by one dispatcher task, because the policy is asked asynchronously and `publish` must not wait for it; a single task keeps every party's events in the order they were published. The topic subscriptions above, which the runtime itself uses, are served directly and receive everything on their topics.
*   **Decoupling**: The `EventBus` allows components like the REST server or TUI to react to state changes without being tightly coupled to the controllers.

### Streams and their snapshot

The REST server exposes two server-sent event streams: the global one (`GET /events`, carrying `GlobalEvent`s) and a channel's (`GET /sessions/{id}/channels/{cid}/events`, carrying `Envelope`s). Both accept `?snapshot=true` and then begin with a named `snapshot` event describing the state they start from — the channel's members plus the last 20 messages (with `has_more`, older ones over `GET .../messages`), or every session with its channels (issue 34).

The order matters: the server subscribes *first* and reads the state afterwards, so nothing that happens while the snapshot is assembled is lost. The price is that an event may arrive twice, once in the snapshot and once as an event; clients drop duplicates by `MessageId` or entity id. When a client falls behind and events are lost, a client that asked for snapshots is sent a fresh one instead of a note that something is missing — on both streams, through the shared `sse::resync_on_lag` (issue 38). The global stream is filtered by what the user may read: its snapshot through the store (issue 11), its events through `subscribe_as` (issue 39).

Global events name sessions and channels by id alone. A client follows them with `GET /sessions/{id}`, which answers in the snapshot's own form (`SessionSnapshot`, built by the same `describe_session`, without starting a stopped session). The web UI's session list works that way (issue 35): it takes its state from the global stream's snapshot, removes a session on `SessionDeleted`, takes a new name from `SessionUpdated`, and fetches the session again on any event that changes its channels or their members; starting and stopping are ignored. Both of the web UI's streams go through one `openEventStream` (`src/stream.ts`), so snapshot, reconnect on `expired` and the session check on a broken connection are handled in one place.

A stream does not outlive the token that authenticated it (issue 33). `require_auth` sees a stream once, when it connects, and never again, so the server ends the response itself — a margin of five seconds before the token expires, announced by a named `expired` event. `EventSource` reconnects, the new request is authenticated like any other, and the snapshot it asks for restores whatever happened in the gap; a client that is no longer entitled gets a `401` there instead. Every stream also begins with an SSE `retry` field of three seconds, the delay a client waits after an *unexpected* break; on `expired` the web UI reconnects at once, so the planned handover does not interrupt the output.

Streams authenticate through `require_stream_auth`, which differs from `require_auth` in one point: it never renews the session. A renewing stream would keep its own session alive for as long as anything holds it open — a tab in the background, or a client that is not a browser at all — which is precisely what the renewal rules are meant to bound. A stream therefore consumes a session; what extends one is `POST /auth/refresh`.

## Logging

All crates log through `tracing`; records from dependencies that still use `log` are forwarded into it. Binaries install the subscriber with `caelumora_runtime::logging::init`: `RUST_LOG` selects the records, `AppConfig`'s `logging` section the format (`text`, the default, or `json`) and an optional file. The REST server logs to stderr, the TUI to `local/tui.log` so that log lines do not break its prompt.

A message crosses several tasks — REST handler, `UserActor`, `ChannelLoop`, moderator, `AgentActor` — connected by channels, so `tracing`'s span nesting alone cannot follow it. The ids of the `EventContext` do: every span that works on an envelope carries them as fields named like the `EventContext` fields (`span_id`, `event_id`, `correlation_id`, `depth`). There are three kinds of span:

*   `request` — one REST request, with `method`, `uri`, the authenticated `user_id`, and the `span_id` of the envelopes the request started (`UserActor::send_message` builds the envelopes and returns it).
*   `envelope` — handling one envelope in the `ChannelLoop` or an idle agent, with session, channel, sender and event kind.
*   `turn` — one iteration of an agent's cognitive cycle, with agent and state and the ids of the turn's `EventContext`; an idle or sleeping agent runs in an `agent` span instead.

Searching the log for one `span_id` therefore finds the request, the routing and the agent's turn that answered it. Message content appears only at `trace` level.

## Error Handling

Errors carry what failed, and only what may be shown goes to users (issue 02).

*   **Domain errors name their subject.** `ComponentError::Build` wraps every failure of a component graph with the component's id and kind; `LLMError` names the endpoint (credentials stripped), the model and the backend's own explanation (`Connection`, `Rejected`, `Backend`, `InvalidResponse`).
*   **What users may see** is decided in one place, `caelumora_runtime::error::user_message`: it looks through an error chain for a domain error written to be read (`LLMError`, `ComponentError`, `ToolError`, `TemplateError`, `StoreError` other than `Backend`) and returns its message; anything else stays in the log.
*   **A failing turn ends the turn, not the agent.** If an error occurs while an `AgentActor` is in a turn — an unreachable model, a fail-closed component, a failing tool executor — the agent logs it, discards the turn's unfinished work and emits `ActorEvent::TurnFailed { message }`, then returns to `Idle`. Moderators forward it to the users; it is broadcast to the UI but never written to the history, so it cannot end up in a model's context ([15](issues/15-disallowed-tool-errors-to-llm.md) is about errors the model *should* see).
*   **REST errors** are RFC 9457 problem details (`application/problem+json`) with two extension members: `code`, a stable identifier (also the last segment of `type`, `urn:caelumora:problem:<code>`), and `request_id`, which also appears as the `x-request-id` header and on the request's log span. Client errors state their cause; server errors state it only if `user_message` allows, and otherwise point to the log. `ensure_session` distinguishes a session that does not exist (`404`) from one that cannot be started (`500`, naming e.g. the broken component).

## Security Model

Security in Caelumora is handled through a combination of context and explicit checks.

*   **`SecurityContext`**: Carries information about the initiator and sender of an event, along with the session and channel context.
*   **`SecurityManager`**: A trait responsible for authorizing actions based on the `SecurityContext`.
*   **`ActionType`**: An enumeration of all securable actions (e.g., `GetAgent`, `UpdateSession`, `ListPersonalities`, `CancelChannel`) used by the `SecurityManager` for policy decisions. An action on a stored entity carries the entity's `SecurityAttributes`, not the entity.

One `SecurityManager` is built per process and handed to everything that checks: the stores, the `SecretManager` and the `RuntimeManager`, from which the `ToolExecutor` and every `ChannelLoop` take it. It is a constructor argument rather than something built where it is needed, so that a deployment's policy cannot be silently replaced by a default in one corner of the runtime. Which one is chosen by `AppConfig`'s `security` section: `ownership` (the default) or `allow_all` (`DummySecurityManager`, for tests and development). A policy outside the binary is [24](issues/24-opa-security-manager.md).

### Ownership

Every stored entity — session, channel, agent, personality, session template, preconfigured component — carries `SecurityAttributes`: an `owner_id` and a list of `contributors` (issue 11). It is the one place ownership is recorded. A session and its channels are owned by the user they were created for, with their users and agents as contributors; an agent is owned by whoever created it and contributes to its own definition; initial data belongs to the system. Entities stored before owners were recorded have an empty owner and count as the system's.

The `OwnershipSecurityManager` decides by those attributes and the context's initiator — the person an action was started for, also when an agent carries it out:

*   the owner may do everything, contributors may read and use (writing into a session, cancelling or waking a channel), and what belongs to the system everybody may read and use;
*   creating is only allowed with oneself as owner, changing and deleting only for the owner;
*   the runtime acting on its own uses `SecurityContext::system()` (`ActorId::System`), which may do everything — deliberately, never as a default: an empty context has no rights.

The stores ask the same way for every entity kind (`store_impl::access`): reads with the attributes as stored, and an entity the caller may not read is reported as not found, so ids cannot be probed; lists are filtered by what the caller may read; changes and deletions are checked against the attributes *as stored*, never those of a new value, which the caller controls; creations against the new entity's. The `EntityStore` trait itself is unchanged. What is not a store call asks first and acts afterwards: `SessionManager::open_session` checks a running session like a stored one, and `delete_session` checks before it stops anything; renaming stores first — which is where it is checked — and changes the live entity only then.

Not decided yet: roles such as an administrator, and richer access lists ([23](issues/23-acls-in-security-attributes.md)); and what happens to an owner's entities when the owner is removed (for now they stay).

### Confidentiality

What may travel from one channel to another is decided by confidentiality levels (`caelumora_model::confidentiality`, issue 61), not by who takes part in the channels: a channel is a construct of a certain trustworthiness, and the decision must not change when someone joins or leaves. A level is a reach, ordered `public < session < channel < private`: anywhere, the channels of the same session, only the one channel, never beyond the actor. Every channel has one of the first three (`Channel::confidentiality`, default `session`), set by its owner when creating it - in the session template or the request.

Labels on the parts of a message say how far each may travel: the labels of a tool call (`ToolConfidentiality`, above) and `Message::reasoning_confidentiality`. The agent labels the reasoning with the most confidential of the personality's `reasoning_confidentiality` (default `public`), `private` once the turn saw the agent's private notes (`TurnContext::saw_private_knowledge`), and the content labels of the calls it leads to - of the message's own calls once it is complete, and while it streams of every tool the agent could still call, because reasoning comes before the call. Knowledge read from other channels adds nothing: rule 2 lets a channel read only what reaches no further than its own contents. The answer text itself is not labelled.

Publishing a labelled part to a channel is `ActionType::Publish { label, actor, channel }`; the ownership policy allows it if the channel admits the label (`ChannelClassification::admits`: the label reaches no further than the channel's contents). The agent asks once per turn and level, in its owner's name, when the turn begins, and keeps the answer as the turn's `Publication`, by which `TurnContext::send_event` redacts every message it sends. `private` is never published, whatever a policy says - it is not asked for. Since the decision depends only on the label, the actor and the channel, it holds for everyone who joins later.

Reading another channel's notes and history is `ActionType::ReadChannel { source, reader }` with both channels' `ChannelClassification` (session, channel, level), asked in the name of the reading channel's owner. The ownership policy allows it if the reader lies within the source's reach and its level is at least the source's (`ChannelClassification::readable_from`): an agent passes on what it reads to its channel, so a `public` channel reads only public contents, and a `channel`-level channel is read by no other. Compartments beside the levels are [64](issues/64-confidentiality-compartments.md), checking who may join a channel against its level is part of [46](issues/46-channel-participants.md).

### Authentication in the REST server

Every route except `POST /auth/login` and `POST /auth/logout` requires a logged-in user. Login verifies username and password through the `UserManager` (see *User Providers*) and answers with a signed JWT (HS256) in an `HttpOnly`, `SameSite=Strict`, `Secure` cookie. There is no server-side session store; the token is the session. The `require_auth` middleware (`caelumora-rest-server/src/auth.rs`) verifies the cookie on each request and puts an `AuthenticatedUser` into it, from which handlers derive their `SecurityContext`. A cookie rather than an `Authorization` header, because `EventSource` cannot send headers and an `HttpOnly` cookie is out of reach of injected scripts.

Sessions are renewed by use: any request past half a token's lifetime gets fresh cookies. Server-sent events cannot do that — one long response cannot set headers again, and their reconnects deliberately do not renew either (see *Streams and their snapshot*) — so the web UI calls `POST /auth/refresh` on a timer *while its tab is visible*, which is what keeps a session alive for someone who is only watching a channel (issue 31). Next to the token cookie the server sets `caelumora_session_expires`, deliberately without `HttpOnly`: it holds only the expiry, and lets the UI (and any other tab) see when the next renewal is due.

A JWT cannot be revoked, so its validity is bounded instead: tokens are short-lived (`auth.token_lifetime_secs`, default 15 minutes) and renewed once past half their lifetime, and only on renewal is the `UserManager` asked whether the user still exists. The `auth_time` claim survives renewal, so a login ends after `auth.max_session_secs` (default 12 hours) regardless. The bound holds for the event streams too, because they end with the token they were opened with (issue 33). The signing key is the secret named by `auth.jwt_secret`, resolved through the secret chain; if it is missing, a random key is generated at startup and every restart logs everybody out.

## Human-in-the-Loop Approvals

Caelumora supports a mechanism for manual user approval of sensitive or impactful tool executions. The process is asynchronous and non-blocking, allowing the agent to suspend its execution while waiting for human input.

*   **Asynchronous Approval Process**: 
    1.  **Interruption**: When the `AgentActor` determines that a tool call requires approval, it suspends its cognitive cycle, persists its current state (including pending tool calls), and transitions to the `WaitingForApproval` state.
    2.  **Request**: The agent emits a `ToolExecutionRequested` event. The `Moderator` routes this event to all human participants (`UserActor`s) in the channel.
    3.  **User Response**: A user's "Approve" or "Deny" response is received via the REST API, which triggers an `ApprovalResponded` global event.
    4.  **Routing**: The `ChannelLoop` captures the global event and routes a corresponding `ToolExecutionGranted` or `ToolExecutionDenied` event back to the specific `AgentActor`.
    5.  **Resumption**: The `AgentActor` receives the response, restores its state, executes the tool calls (if granted), and resumes its cognitive cycle. 
    6.  **Message Queueing**: Any messages received while the agent is in the `WaitingForApproval` state are queued and processed sequentially after the agent resumes, ensuring context integrity.

*   **Policy before approval**: a call the `SecurityManager` does not permit (`ActionType::CallTool`, via `ToolExecutor::is_permitted`) is refused before any approval is requested, so users are not asked about calls that would be refused anyway. `execute` checks again.
*   **Every call ends with a result the model can act on** (issue 15). A successful call's result is the tool's output. Every other outcome is given to the model as a JSON object `{"status": ..., "message": ...}` whose message says what happened and what to do next — `denied` (by a user), `forbidden` (by policy, without the reason, which stays in the log), `timed_out` (no approval in time), `invalid_arguments`, `failed` (with the tool's error) and `unknown_tool` (with the tools that do exist). The same status is stored as `ToolCallResult::status`, so the UI and an audit trail can tell refusals and failures from results. The wording lives in `crate::tool` (`ToolError::to_result`, `denied_result`, `timed_out_result`, `forbidden_result`).

## Storage and Persistence

Caelumora uses a tiered storage approach to separate static definitions, dynamic identities, and runtime state.

### `EntityStore`
The `EntityStore` trait (in `caelumora-model`) manages the persistence of long-lived domain entities. It handles:
*   **Personalities**: Static definitions of agent traits.
*   **Templates**: `SessionTemplate` and `AgentTemplate` used for instantiation.
*   **Identities**: `Agent`, `Session`, and `User` records.

Implementations include `MemoryStore` (for testing) and `JsonFileStore` (for simple file-based persistence).

### `StateStore`
The `StateStore` trait (in `caelumora-model`) is dedicated to managing the frequently changing runtime state of agents and sessions.
*   **`ChannelState`**: the channel's history.
*   **`AgentChannelState`**: an agent in one channel - its history and context, its public notes (later also its private notes), the texts of its prompt parts by digest, and its turn state (processing state, pending tool calls, plan, the turn's messages, notes not yet taken over).
*   **`AgentState`**: the agent itself, across its channels and - for agents of user or global scope - across sessions: so far its memory (`agent_memory`). One owner per agent and process: the `RuntimeManager`'s `AgentStates` hands every `AgentActor` of the agent, in whatever channel or session, the same `AgentStateHandle`. A turn does not hold the state; `TurnContext` gets the handle in its constructor and offers only generic access - a snapshot (`agent_state().current()`), staging `AgentStateChange`s (`stage_agent_change`) - and knows nothing of what the state holds: the logic of a kind of state is a wrapper around the turn, such as `agent_memory::AgentMemory` (recall, remember, forget), which the component and anything else use. `TurnContext::commit_agent_state` applies the staged changes to the state as it is when the turn completes, under the handle's lock, and stores, and the handle then announces `GlobalEvent::AgentStateChanged` (with the agent's attributes, without content) - so no actor overwrites another's changes, and every next turn sees them (issue 62). Numbers for new memory entries are handed out by the handle at once, since turns in several channels may remember at the same time.

Every kind of state has exactly one writer: `ChannelState` its `ChannelLoop`, `AgentChannelState` the `AgentActor` of that agent and channel, `AgentState` its handle. That holds because a session starts only once, however many requests ask for it at the same time (`SessionManager::load_session` holds a lock per session id); the JSON stores additionally write every file through a temporary file of its own.
*   **`SessionState`**: state of a live session.

Entity store and state store are configured separately and may be different stores. Deleting a session or a channel therefore deletes its state through the `StateStore` (`delete_session_state`, `delete_channel_state`, which removes the channel's state and that of every agent in it), never as a side effect of `EntityStore::delete_session`.

### `SessionContext`
Provides necessary dependencies (stores, session ID, owner) for instantiating live objects from templates.

### Terms
The words for what an agent remembers are used in one sense each (issue 19, where the principles behind them are recorded). "Branches" means: follows the branch of the conversation it came from ([08](issues/08-retry-and-edit-past-answers.md)).

| Term | Meaning | Scope | Lifetime | Branches |
|---|---|---|---|---|
| **History** | The record of what happened, never rewritten or compressed: per channel (`ChannelState`), and per agent in a channel with what each own message was produced from. | channel / agent in channel | session | yes |
| **Context** | The messages an agent's model is given as the conversation (`MessageContext`); derived from the history, may differ from it (compression, [16](issues/16-context-compression.md)). | agent in channel | session | yes |
| **Prompt part** | Text given besides the conversation - the system prompt, notes - transient, recorded by digest (see *Prompt parts* above). | turn | transient | - |
| **Model input** | What one call of the model is given: prompt parts, context and the turn's messages; reconstructable through `ContextRef` ("what the agent saw"). In code still called "context" ([50](issues/50-turn-context-naming.md)). | one call | transient | - |
| **Turn state** | The working state of a turn: plan, pending tool calls, the turn's messages, the changes staged to the agent's states. | agent in channel | turn | discarded |
| **Notes** | Entries in an agent's public note chain of one channel, published there, with provenance; only the author writes and revokes ([17](issues/17-information-sharing-within-session.md)). | agent in channel | session | yes |
| **Session memory** | The union of the active public note chains of all agents in all channels of a session - a view, not a store. | session | session | per channel |
| **Agent memory** | What an agent remembers across its channels and sessions: a chain in its `AgentState` (component `agent_memory`, tools `remember` / `forget`), each entry carrying the classification of the channel it was written in and recalled only where that channel may be read ([62](issues/62-json-store-concurrent-writes.md)). | agent | beyond the session | no |
| **Private notes** | An agent's private note chain in one channel - the same mechanism as notes, readable only by the agent, carrying the confidentiality of that channel ([51](issues/51-agent-private-notes.md)). | agent in channel | session | yes |
| **Agent session memory** | The union of an agent's active private note chains across its channels - a view, readable only by the agent, each note only where its confidentiality allows. | agent in session | session | per channel |
| **Agent knowledge** | What an agent keeps across sessions, committed from notes ([18](issues/18-knowledge-sharing.md)). | agent | permanent | no |
| **Shared knowledge** | Knowledge shared by several agents or users ([18](issues/18-knowledge-sharing.md)). | group / global | permanent | no |
| **Workspace** | Documents several channels edit together; outside the branches, every change an effect ([20](issues/20-transactional-effects.md)). | session or beyond | until deleted | no |

Two technical words are not tiers: **state** is the container the `StateStore` keeps (history, context, turn state and notes live *in* state), as opposed to **entities** in the `EntityStore`; and the structs named `…Context` (`SecurityContext`, `EventContext`, `ComponentContext`, `SessionContext`, `CommandContext`, `TurnContext`) are *operation contexts*, the circumstances an operation runs in, unrelated to the model's context. In prose, "context" alone always means the model's context; the others are named in full.

## Templates and Identities

*   **Templates (`SessionTemplate`, `AgentTemplate`)**: Declarative definitions used to instantiate sessions and agents. They specify the initial configuration, participants, and personalities.
*   **Identities (`Agent`, `User`)**: Represent the persistent entities within the system.

## Relations
*   Each **Session** can contain one or more **Channels**.
*   Each **Channel** has multiple **Participants** (Actors).
*   A single **Agent** can be a participant in multiple sessions/channels.
*   Within each session, each agent maintains its own `AgentState` and cognitive cycle within the `AgentActor`.