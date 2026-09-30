# Configuration

Both binaries - the REST server and the TUI - read one YAML file, given with
`--config` (`AppConfig`, `caelumora_model::config`). Without it, every section
takes its default. `examples/config.yaml` is the configuration for local
development, with every section and its default in comments.

Every section may be left out and then takes its default. A key the
configuration does not know is an error rather than ignored, so that a
misspelt section doesn't silently fall back to its default (issue 96).

| Section | What it configures | Default |
|---|---|---|
| `persistence` | Where the `EntityStore` (`entity_store`) and the `StateStore` (`state_store`) keep their data: `type: json` with a `path` (one JSON file per document), or `type: memory`. The two may differ. See [STORAGE.md](STORAGE.md). | `memory` for both - gone when the process ends |
| `seed` | Files of demo data - personalities and session templates - put into the entity store at startup, only what it does not have yet, by id (issue 105). | none |
| `secrets` | The chain of secret providers, tried in order: `type: file` with a `path` (a flat JSON key/value file), `type: env`. See [SECURITY.md](SECURITY.md). | `local/secrets.json`, then the environment |
| `users` | The chain of user providers. The only one so far is `accept_any`, a development stand-in that accepts any username and password. See [SECURITY.md](SECURITY.md). | none - nobody can log in |
| `auth` | How the REST server keeps a browser logged in: `jwt_secret` (the name of the secret holding the signing key), `token_lifetime_secs`, `max_session_secs`, `secure_cookie`. See [SECURITY.md](SECURITY.md). | `CAELUMORA_JWT_SECRET`, 15 minutes, 12 hours, `true` |
| `security` | The access policy: `type: ownership` or `type: allow_all` (tests and single-user development only). See [SECURITY.md](SECURITY.md). | `ownership` |
| `server` | Where the REST server listens (`address`). | `127.0.0.1:3000` - this machine only, as long as `accept_any` may be enabled |
| `logging` | The log `format` (`text` or `json`) and an optional `file`. What is logged is selected with `RUST_LOG`. See [CODING-GUIDELINES.md](CODING-GUIDELINES.md). | `text`; stderr (REST server) or `local/tui.log` (TUI) |
| `commands` | The user commands every session has, as components (`component_nodes`); a session template can add its own. An empty list switches them off. See [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md). | `help_command` and `wakeup_command` |
| `component_defaults` | Components that replace the built-in default of a facet of which an agent has exactly one (`ContextProvider`, `ToolExecutor`), for every personality that does not configure its own. One component per such facet, checked when the `RuntimeManager` is made. See [COMPONENTS.md](COMPONENTS.md). | the built-in defaults |

## The model

The language model is not part of `AppConfig` yet. The only client,
`OllamaClient`, reads the environment when it is made:

| Variable | Meaning | Default |
|---|---|---|
| `OLLAMA_BASE_URL` | The Ollama server | `http://127.0.0.1:11434` |
| `OLLAMA_MODEL` | The model every agent uses | `gemma4:26b` |
| `OLLAMA_THINK` | Whether the model is asked to reason (`true`/`false`); unset leaves it to the model | unset |
