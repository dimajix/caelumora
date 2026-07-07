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
*   **Traits**: Components implement specific traits like `MessageProcessor`, `PerceiveInProcessor`, `PlanPreProcessor`, or `ContextProvider` to participate in different stages of the agent loop.

## Agent Cognitive Cycle

The agent cognitive cycle (implemented within `AgentActor`) defines how an agent processes information and generates responses. It typically follows these stages:

1.  **Perceive**: Incoming messages are processed by "perceive" components (e.g., adding to history, updating context).
2.  **Plan**: The agent decides what to do next. This may involve interacting with an LLM, planning tool calls, or generating internal thoughts.
3.  **Execute**: If the plan involves tool calls, they are executed, and the results are fed back into the loop.
4.  **Emit**: The final response messages are generated and emitted to the channel.

The loop is stateful, maintaining an `AgentState` which includes `MessageHistory` and `MessageContext`.

## Session, Channel, and Actor Design

Caelumora uses an actor-based model for communication within sessions.

### Session and SessionLoop
A **Session** represents a high-level container for a specific interaction context. The `SessionLoop` manages multiple channels within that session.

### Channel and ChannelLoop
A **Channel** is a specific communication medium within a session (e.g., a chat room). The `ChannelLoop` is responsible for:
*   Managing the lifecycle of **Actors** participating in the channel.
*   Routing **Envelopes** (events) between actors.
*   Applying a **Moderator** to control message flow and determine which actor speaks next.

### Actor and ActorId
An **Actor** is any entity that can send and receive events in a channel.
*   **`Actor` trait**: Defines the interface for all actors, including an `async fn run` method.
*   **`ActorId`**: Uniquely identifies an actor, which can be a `User` or an `Agent`.
*   **`Envelope`**: The unit of communication, containing a timestamp, security context, event context, and the actual `ActorEvent`.

### Moderators
Moderators control how messages are routed and who is allowed to respond. Implementations include:
*   **Broadcast**: Forwards messages to all participants.
*   **Round Robin**: Cycles through participants in a fixed order.
*   **Random**: Picks a random participant to respond.

## Controllers and State Management

Caelumora uses `Controller` types (e.g., `SessionController`, `AgentController`) for managing the live state of entities. 
*   **Live State**: Controllers use `ArcSwap` to provide atomic, lock-free access to the current state of an entity.
*   **Persistence**: Controllers are linked to the `EntityStore`. Any state modifications made through a controller (e.g., updating a name) are automatically propagated to the persistent store, ensuring data consistency across restarts.
*   **Eventing**: Controllers are responsible for publishing update events via the `EventBus` when state changes occur.

## Eventing and Observation

The `EventBus` (in `caelumora-runtime`) provides a centralized mechanism for observing changes across the system.

*   **`GlobalEvent`**: Represents any significant change in the system, such as creation, update, or deletion of sessions, channels, agents, or users.
*   **Subscriptions**: Clients can subscribe to specific `SubscriptionTopic`s (e.g., `All`, `Session(id)`, `Agent(id)`) to receive relevant `GlobalEvent`s via asynchronous channels.
*   **Decoupling**: The `EventBus` allows components like the REST server or TUI to react to state changes without being tightly coupled to the controllers.

## Security Model

Security in Caelumora is handled through a combination of context and explicit checks.

*   **`SecurityContext`**: Carries information about the initiator and sender of an event, along with the session and channel context.
*   **`SecurityManager`**: A trait responsible for authorizing actions based on the `SecurityContext`.
*   **`ActionType`**: An enumeration of all securable actions (e.g., `GetAgent`, `UpdateSession`, `ListPersonalities`) used by the `SecurityManager` for policy decisions.

## Human-in-the-Loop Approvals

Caelumora supports a mechanism for manual user approval of sensitive or impactful tool executions.

*   **`ApprovalManager`**: A trait (in `caelumora-runtime`) responsible for managing the lifecycle of approval requests. It maintains a registry of pending approvals and provides a bridge between asynchronous tool execution and user response.
*   **Approval Process**: 
    1.  When a `ToolExecutor` encounters a tool marked with `requires_approval: true`, it pauses execution and registers an `ApprovalRequest` with the `ApprovalManager`.
    2.  The `ApprovalManager` publishes an `ApprovalRequested` event via the `EventBus`.
    3.  The UI receives this event and prompts the user for "Approve" or "Deny".
    4.  The user's response is sent back via a REST API call, which triggers the `ApprovalManager` to resolve the pending request.
    5.  The `ToolExecutor` resumes execution once the approval is received (or returns an error if denied).

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
*   **Agent State**: Includes the current cognitive state, message history, and context.
*   **Session State**: Includes metadata and operational state of a live session.

### `SessionContext`
Provides necessary dependencies (stores, session ID, owner) for instantiating live objects from templates.

## Templates and Identities

*   **Templates (`SessionTemplate`, `AgentTemplate`)**: Declarative definitions used to instantiate sessions and agents. They specify the initial configuration, participants, and personalities.
*   **Identities (`Agent`, `User`)**: Represent the persistent entities within the system.

## Relations
*   Each **Session** can contain one or more **Channels**.
*   Each **Channel** has multiple **Participants** (Actors).
*   A single **Agent** can be a participant in multiple sessions/channels.
*   Within each session, each agent maintains its own `AgentState` and cognitive cycle within the `AgentActor`.