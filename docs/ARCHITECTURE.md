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

## Detailed documents

*   [COMPONENTS.md](COMPONENTS.md) - components, facets and how they are wired.
*   [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md) - tools, their execution and approval, user commands.
*   [AGENT-TURN.md](AGENT-TURN.md) - the agent's cognitive cycle, the turn, what an agent saw, prompt parts.
*   [MEMORY.md](MEMORY.md) - notes, private notes, agent memory, and the terms for what an agent remembers.
*   [CHANNELS.md](CHANNELS.md) - sessions, channels, actors, participants, moderators and mentions.
*   [EVENTS-AND-STREAMS.md](EVENTS-AND-STREAMS.md) - the event bus and the server-sent event streams.
*   [STORAGE.md](STORAGE.md) - entity and state store, controllers, several instances.
*   [SECURITY.md](SECURITY.md) - security manager, ownership, users, authentication, secrets.
*   [CONFIDENTIALITY.md](CONFIDENTIALITY.md) - confidentiality levels and labels.
*   [CONFIGURATION.md](CONFIGURATION.md) - the sections of the configuration.
*   [CODING-GUIDELINES.md](CODING-GUIDELINES.md) - coding style, error handling, logging, tests.

## Templates and Identities

*   **Templates (`SessionTemplate`)**: Declarative definitions of sessions: every actor of the session - users, agents with their personality and scope, approvers with their strategy - each with a name, and the channels, which refer to them by that name. A session is made from a template in two steps (issue 88). The model says what each participant is for the session (`ParticipantTemplate::request`: "user" is the session's owner, an agent is looked up by name in its scope, an approver is made) and builds the session and its channels from the participants it is given (`SessionTemplate::build`) - without touching a store. `SessionManager::create_from_template(ctx, template_id, name)` does the rest: it reads the template, finds the owner and the agents or makes the agents that are missing (only when the lookup finds none, never because it failed), builds, stores and starts the session. REST (`POST /sessions`) and the TUI both call it.
*   **Identities (`Agent`, `User`)**: Represent the persistent entities within the system.

## Relations
*   Each **Session** can contain one or more **Channels**.
*   Each **Channel** has multiple **Participants** (Actors).
*   Sessions and channels hold their users and agents by reference - `Participant::User(UserId)`, `Participant::Agent(AgentId)` - not as copies (issue 120). The records live in the `EntityStore`: a channel gets their controllers from the `ControllerManager` when it builds their actors, and leaves out (with a warning) one that no longer exists - it runs without it; REST reads the names of a stopped session's participants from the store the same way, since taking part in a session makes a name known to everyone in it. An approver has no record of its own and is held whole.
*   A single **Agent** can be a participant in multiple sessions/channels.
*   Within each session, each agent maintains its own `AgentState` and cognitive cycle within the `AgentActor`.
