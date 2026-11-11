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

- **`caelumora-domain`**: Core domain entities and data structures.
- **`caelumora-security`**: Who may do what and what may flow where: the security context, the
  policies and the secrets.
- **`caelumora-store`**: The stores of the entities, the states, the histories and the transcripts.
- **`caelumora-kernel`**: The execution engine: sessions, channels, event routing and the views.
- **`caelumora-api`**: The kernel's API: what a library of the agent side is given and declares.
- **`caelumora-agents`**: What an agent does in a channel: its cognitive cycle, tools and
  prompts; the model it talks to is offered by the plugin `caelumora-model`.
- **`caelumora-rest-api`**: The request and response types of the REST interface.
- **`caelumora-rest-server`**: REST interface for interacting with the system, as a crate the
  program links.
- **`caelumora-app`**: The program: the REST interface under `/api` and the web UI.
- **`caelumora-tui`**: Terminal User Interface for Caelumora.
- **`caelumora-logging`**: The setup of the log, shared by `caelumora-app` and `caelumora-tui`.
- **`caelumora-rest-client`**: Reserved for a client of the REST interface; empty so far.

Next to the workspace, **`caelumora-web-ui`** is the browser frontend (React, TypeScript) for the REST server.

## Documentation

- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**: The design of the system, modules, and data flow, with the detailed documents in [docs/](docs/).
- **[AGENTS.md](AGENTS.md)**: Guidelines for AI coding agents and contributors.
- **[licenses/](licenses/)**: The third-party dependencies with their licenses, [BACKEND.md](licenses/BACKEND.md) for the Rust crates and [FRONTEND.md](licenses/FRONTEND.md) for the web UI.
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
cargo run -p caelumora-tui
```

The TUI is a program of one person, in the directory it is started in (issue 554). Without a
configuration it keeps what the person writes in `~/.config/caelumora` - skills, personalities,
bundles and templates, each a file by its name - and what it writes itself in
`~/.local/share/caelumora`, a directory per session; the directories of XDG move them. On the first
start it writes there a template `default` and the personality of its assistant. The directory it
was started in is the workspace of its session, and its commands run there with the person's own
environment; every session of one directory lies in that directory's project. It acts for the user
of the system, or for the one `--user <name>` names, and starts a session from the template
`default`, or from the one `--template <name>` names. What `~/.config/caelumora/config.yaml` says
lies over these defaults, key by key (see `docs/CONFIGURATION.md`); `--config <file>` replaces
them, as `--config examples/config.yaml --template sassy-session` does with the demo data of
`examples/seed`. The TUI writes its log to `~/.local/state/caelumora/tui.log`.

### Running the Program

```bash
cargo run -p caelumora-app -- --config examples/config.yaml
```

It serves the web UI at `http://127.0.0.1:3000/` and the REST API under `/api`. The server requires
a login. Users come from the providers in the config's
`users` section; without one, nobody can log in. `examples/config.yaml`
enables the `accept_any` provider for local development — it accepts any
username and password and must never be enabled on a server reachable from a
network. For real users, the `file` provider reads them, with hashed
passwords, from a YAML file (see `docs/SECURITY.md`); `examples/users.yaml`
is one to try it with, each user's password their name. Session tokens are signed with the secret `CAELUMORA_JWT_SECRET`
(at least 32 bytes, e.g. in `local/secrets.json`); without it, every restart
logs everybody out.

Logging goes to stderr. Use `RUST_LOG` to select what is logged, e.g.
`RUST_LOG=info,caelumora_kernel=debug`; the `logging` section of the config
switches to JSON or to a file. At `debug`, every line concerning one message
carries the same `span_id`.

## License

Caelumora is licensed under either of

*   Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
    <https://www.apache.org/licenses/LICENSE-2.0>)
*   MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option. The Caelumora Authors are the people named in the Git history of this repository.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
Caelumora by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.

### Third-party licenses

The licenses of the third-party dependencies are listed in
[licenses/BACKEND.md](licenses/BACKEND.md) and [licenses/FRONTEND.md](licenses/FRONTEND.md). Both
are generated by `licenses/generate.py`; run it again whenever `Cargo.lock` or
`caelumora-web-ui/package-lock.json` changes. With
`--bundle`, it writes the license texts of everything that goes into the program to
`target/THIRD-PARTY-LICENSES.txt`, to be distributed with it (needs network access). With
`--sbom`, it writes a software bill of materials in CycloneDX JSON to `target/caelumora.cdx.json`.
Both need the installed packages of the web UI (`npm ci` in `caelumora-web-ui`).
