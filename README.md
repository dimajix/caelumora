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
- **`caelumora-runtime`**: The execution engine, handling event routing and agent cognitive cycles.
- **`caelumora-rest-server`**: REST interface for interacting with the system.
- **`caelumora-client-api`**: Common traits and types for client implementations.
- **`caelumora-local-client`**: Direct-link client for internal use and testing.
- **`caelumora-tui`**: Terminal User Interface for Caelumora.

## Documentation

- **[ARCHITECTURE.md](ARCHITECTURE.md)**: Detailed design of the system, modules, and data flow.
- **[AGENTS.md](AGENTS.md)**: Guidelines for AI coding agents and contributors.
- **[issues/INDEX.md](issues/INDEX.md)**: Open issues — one line per issue, with the reasoning in `issues/<number>-<slug>.md`.

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

The TUI writes its log to `local/tui.log`.

### Running the REST Server

```bash
cargo run -p caelumora-rest-server -- --config examples/config.yaml
```

The server requires a login. Users come from the providers in the config's
`users` section; without one, nobody can log in. For local development,
uncomment the `accept_any` provider in `examples/config.yaml` — it accepts any
username and password and must never be enabled on a server reachable from a
network. Session tokens are signed with the secret `CAELUMORA_JWT_SECRET`
(at least 32 bytes, e.g. in `local/secrets.json`); without it, every restart
logs everybody out.

Logging goes to stderr. Use `RUST_LOG` to select what is logged, e.g.
`RUST_LOG=info,caelumora_runtime=debug`; the `logging` section of the config
switches to JSON or to a file. At `debug`, every line concerning one message
carries the same `span_id`.

## License

*See individual crates or project root for license information.*
