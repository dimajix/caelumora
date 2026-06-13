# Caelumora

Caelumora is a flexible and extensible multi-agent system framework designed for building sophisticated agentic workflows. It leverages an actor-based communication model to manage interactions between human users and AI agents across various channels and sessions.

## Core Features

- **Multi-Agent Orchestration**: Coordinate multiple agents within shared communication channels.
- **Actor-Based Architecture**: Robust event-driven communication using a session/channel/actor model.
- **Pluggable Components**: Easily extend agent capabilities (perception, planning, execution) through a trait-based component system.
- **Flexible Interfaces**: Support for REST API, TUI, and local client integrations.
- **Tiered Storage**: Clear separation between static templates and dynamic session state.

## Project Structure

The project is organized as a Rust workspace with the following crates:

- **`caelumora-model`**: Core domain entities and data structures.
- **`caelumora-runtime`**: The execution engine, handling event routing and the `AgentLoop`.
- **`caelumora-rest-server`**: REST interface for interacting with the system.
- **`caelumora-client-api`**: Common traits and types for client implementations.
- **`caelumora-local-client`**: Direct-link client for internal use and testing.
- **`caelumora-tui`**: Terminal User Interface for Caelumora.

## Documentation

- **[ARCHITECTURE.md](ARCHITECTURE.md)**: Detailed design of the system, modules, and data flow.
- **[AGENT.md](AGENT.md)**: Guidelines for AI coding agents and contributors.
- **[TODO.md](TODO.md)**: Current roadmap and pending tasks.

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/) (latest stable version recommended)

### Building the Project

```bash
cargo build
```

### Running Tests

```bash
cargo test
```

### Running the TUI

```bash
cargo run -p caelumora-tui
```

### Running the REST Server

```bash
cargo run -p caelumora-rest-server
```

## License

*See individual crates or project root for license information.*
