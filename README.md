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
- **`caelumora-runtime`**: The execution engine: sessions, channels, event routing and the views.
- **`caelumora-agents`**: What an agent does in a channel: its cognitive cycle, tools, prompts and
  model.
- **`caelumora-rest-api`**: The request and response types of the REST interface.
- **`caelumora-rest-server`**: REST interface for interacting with the system, as a library.
- **`caelumora-app`**: The program: the REST interface under `/api` and the web UI.
- **`caelumora-tui`**: Terminal User Interface for Caelumora.
- **`caelumora-logging`**: The setup of the log, shared by `caelumora-app` and `caelumora-tui`.
- **`caelumora-rest-client`**: Reserved for a client of the REST interface; empty so far.

Next to the workspace, **`caelumora-web-ui`** is the browser frontend (React, TypeScript) for the REST server.

## Documentation

- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**: The design of the system, modules, and data flow, with the detailed documents in [docs/](docs/).
- **[AGENTS.md](AGENTS.md)**: Guidelines for AI coding agents and contributors.
- **[issues/INDEX.md](issues/INDEX.md)**: Every issue, one line each, by area; [issues/OPEN.md](issues/OPEN.md) those still open. The reasoning is in `issues/<number>-<slug>.md`, how issues are kept in [issues/README.md](issues/README.md).

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/) (latest stable version recommended)
- [Node.js](https://nodejs.org/) with npm, to build the web UI

### Building the Project

The program serves the web UI from its binary, so the UI is built first:

```bash
(cd caelumora-web-ui && npm ci && npm run build)
cargo build
```

`CAELUMORA_BUILD_UI=1 cargo build --release` builds the UI as part of the build. Without the UI,
`cargo build -p caelumora-app --no-default-features` builds a program that serves the API alone.

### Running Tests

```bash
cargo test
```

### Running the TUI

```bash
cargo run -p caelumora-tui -- --config examples/config.yaml
```

The TUI starts a session from the template `sassy-session`; `--template <name>`
picks another. It talks in the template's first channel, as the user `--user <name>` names - `user`
unless given -, whom the configured user providers know under that name, with their roles. The
templates and personalities come from the demo data that `examples/config.yaml` seeds
(`examples/seed.yaml`) - without a configuration there are none. The TUI writes its log to `local/tui.log`.

### Running the Program

```bash
cargo run -p caelumora-app -- --config examples/config.yaml
```

It serves the web UI at `http://127.0.0.1:3000/` and the REST API under `/api`. The server requires
a login. Users come from the providers in the config's
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
