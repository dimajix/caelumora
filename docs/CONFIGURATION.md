# Configuration

Both binaries - the REST server and the TUI - read one YAML file, given with `--config`
(`AppConfig`, `caelumora_model::config`). Without it, every section takes its default.
`examples/config.yaml` is the configuration for local development, with every section and its
default in comments.

Every section may be left out and then takes its default. A key the configuration does not know is
an error rather than ignored, so that a misspelt section doesn't silently fall back to its default
(issue 96).

| Section | What it configures | Default |
|---|---|---|
| `persistence` | Where the `EntityStore` (`entity_store`) and the `StateStore` (`state_store`) keep their data: `type: json` with a `path` (one JSON file per document), or `type: memory`. The two may differ. See [STORAGE.md](STORAGE.md). | `memory` for both - gone when the process ends |
| `seed` | Files of demo data - personalities and session templates - put into the entity store at startup, only what it does not have yet, by id (issue 105). | none |
| `secrets` | The chain of secret providers, tried in order: `type: file` with a `path` (a flat JSON key/value file), `type: env`. See [SECURITY.md](SECURITY.md). | `local/secrets.json`, then the environment |
| `users` | The chain of user providers. The only one so far is `accept_any`, a development stand-in that accepts any username and password; it gives the users named under `roles` their roles and everybody else `default_roles`. See [SECURITY.md](SECURITY.md). | none - nobody can log in |
| `auth` | How the REST server keeps a browser logged in: `jwt_secret` (the name of the secret holding the signing key), `token_lifetime_secs`, `max_session_secs`, `secure_cookie`. See [SECURITY.md](SECURITY.md). | `CAELUMORA_JWT_SECRET`, 15 minutes, 12 hours, `true` |
| `security` | The access policy: `type: ownership` or `type: allow_all` (tests and single-user development only). See [SECURITY.md](SECURITY.md). | `ownership` |
| `roles` | The roles, each with the `clearances` it grants - a `level` and at most one `compartment`. Roles given by a user provider and compartments named here must be configured. See [CONFIDENTIALITY.md](CONFIDENTIALITY.md#clearances). | none - clearances are not checked |
| `compartments` | The compartments a classification may name, each with a `name` and the compartments it `reads`; the read relation is closed transitively. Checked when the configuration is read. See [CONFIDENTIALITY.md](CONFIDENTIALITY.md). | none - every classification is general |
| `server` | Where the REST server listens (`address`). | `127.0.0.1:3000` - this machine only, as long as `accept_any` may be enabled |
| `logging` | The log `format` (`text` or `json`) and an optional `file`. What is logged is selected with `RUST_LOG`. See [CODING-GUIDELINES.md](CODING-GUIDELINES.md). | `text`; stderr (REST server) or `local/tui.log` (TUI) |
| `commands` | The user commands every session has, as components (`component_nodes`); a session template can add its own. An empty list switches them off. See [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md). | `help_command` and `wakeup_command` |
| `component_defaults` | Components that replace the built-in default of a facet of which an agent or a channel has exactly one (`LLMClient`, `ContextProvider`, `ToolExecutor`; `Moderator`, `ResponderPolicy`), for every personality or channel that does not configure its own. One component per such facet, checked when the `RuntimeManager` is made. See [COMPONENTS.md](COMPONENTS.md). | the built-in defaults |

## The model

The language model an agent talks to is a component (issue 130): the `LLMClient` facet, of which an
agent has exactly one. A personality may configure its own; every other agent gets the one
`component_defaults` names, or else `ollama_client` with its defaults. There is no separate section
for it, and no environment variable.

```yaml
component_defaults:
  component_nodes:
    - id: model
      kind: ollama_client
      params:
        endpoint:
          url: "http://127.0.0.1:11434"
        model: "gemma4:26b"
        think: true
        reasoning_history: current_turn
        system_messages: only_at_start
```

| Parameter of `ollama_client` | Meaning | Default |
|---|---|---|
| `endpoint` | The Ollama server: its `url`, and its `timeout_secs` - how long a request may take to be answered, a streamed one until its stream begins. Ollama is reached without authentication: an `auth` is refused (see [COMPONENTS.md](COMPONENTS.md#remote-endpoints)) | `url: http://127.0.0.1:11434`, no timeout |
| `model` | The model | `gemma4:26b` |
| `think` | Whether the model is asked to reason; left out, Ollama's default for the model applies | left out |
| `reasoning_history` | Which earlier reasoning the model is given back: `never`, `current_turn` or `always` (issue 63, see [AGENT-TURN.md](AGENT-TURN.md#reasoning-given-back)) | `current_turn` |
| `system_messages` | Where the model keeps system messages: `only_at_start` - any later one is given as a user message - or `anywhere` (issue 49, see [AGENT-TURN.md](AGENT-TURN.md#system-messages)) | `only_at_start` |

An unknown parameter is refused, and the agent does not start.

Before the first call to a model, the client reads the model's control tokens from the server's
`/api/show` and keeps them from being produced by any text it sends (issue 149, see
[AGENT-TURN.md](AGENT-TURN.md#control-tokens-in-the-content)). If they cannot be read, the call
fails.
