# Caelumora

Caelumora is a flexible and extensible multi-agent system framework designed for building sophisticated agentic workflows. It leverages an actor-based communication model to manage interactions between human users and AI agents across various channels and sessions.

## Core Features

- **Multi-Agent Orchestration**: Coordinate multiple agents within shared communication channels.
- **Actor-Based Architecture**: Robust event-driven communication using a session/channel/actor model.
- **Pluggable Components**: Easily extend agent capabilities (perception, planning, execution) through a trait-based component system.
- **External Agents**: Seat agents that run elsewhere in a channel beside your own, reached over the Agent2Agent protocol (A2A 1.0), and limit what they are shown by roles.
- **Flexible Interfaces**: Support for REST API, TUI, and local client integrations.
- **Tiered Storage**: Clear separation between static templates and dynamic session state.

## Project Structure

The project is organized as a Rust workspace with the following crates:

- **`caelumora-model`**: Core domain entities and data structures.
- **`caelumora-runtime`**: The execution engine, handling event routing and agent cognitive cycles.
- **`caelumora-rest-api`**: The request and response types of the REST interface.
- **`caelumora-rest-server`**: REST interface for interacting with the system.
- **`caelumora-tui`**: Terminal User Interface for Caelumora.
- **`caelumora-client-api`**, **`caelumora-local-client`**, **`caelumora-rest-client`**: Reserved for a client interface and its implementations; empty so far.

Next to the workspace, **`caelumora-web-ui`** is the browser frontend (React, TypeScript) for the REST server.

## Documentation

- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**: The design of the system, modules, and data flow, with the detailed documents in [docs/](docs/).
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
cargo run -p caelumora-tui -- --config examples/config.yaml
```

The TUI starts a session from the template `sassy-session`; `--template <name>`
picks another. It talks in the template's first channel. The templates and personalities come from the demo data that
`examples/config.yaml` seeds (`examples/seed.yaml`) - without a configuration
there are none. The TUI writes its log to `local/tui.log`.

### Running the REST Server

```bash
cargo run -p caelumora-rest-server -- --config examples/config.yaml
```

The server requires a login. Users come from the providers in the config's
`users` section; without one, nobody can log in. `examples/config.yaml`
enables the `accept_any` provider for local development — it accepts any
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
