# Configuration

Both programs - `caelumora-app` and the TUI - read one YAML file, given with `--config`
(`AppConfig`, `caelumora_model::config`). `examples/config.yaml` is the configuration for local
development, with every section and its default in comments.

Every section may be left out and then takes its default. Within a section, a key it does not know
is an error when the configuration is read. A section the configuration does not know is a plugin's
(`AppConfig::plugins`, issue 323): the plugin that claims it reads it when it registers
(`RuntimeBuilder::config`), and a section that no plugin claims is an error when the runtime is
built. Either way a misspelt key does not silently fall back to its default.

Only `true` and `false` are booleans; `yes`, `no`, `on`, `off` and `y` are text (YAML 1.2), in the
configuration as in the seed files. So a value that nothing types until later - the parameters of a
backend, the section of a plugin - is read as it is written, and `shell: yes` is refused rather
than taken for a switch.

### Backends

Where a service is served by one of a choice of backends - a storage of workspaces or of artifacts,
a secret provider, a user provider - the backend is named by its `type`, with its parameters beside
it (`BackendConfig`, issue 324):

```yaml
workspaces:
  storage:
    files:
      type: local
      root: "local/workspaces/files"
```

The type names the factory registered for it, the core's or a plugin's (see
[ARCHITECTURE.md](ARCHITECTURE.md#backends)), which reads the parameters. So a type nobody
registers, a parameter its backend does not know or one that does not fit are not refused when the
file is read but when the runtime is built - still at start-up, before anything runs, with a
message naming the backend: `the storage of workspaces 'files' (local)`, or for a chain its
position, `the secret provider 2 (vault)`. An unknown type is answered with the types there are.
The persistence (`persistence`) is named the same way, but is the core's alone.

| Section | What it configures | Default |
|---|---|---|
| `persistence` | Where the `EntityStore` (`entity_store`) and the `StateStore` (`state_store`) keep their data: `type: json` with a `path`, and optionally `journal_floor`, the size in bytes a journal reaches before it is folded into its snapshot; or `type: memory`. The two may differ. The `ComponentStore` is kept where the entity store is. See [STORAGE.md](STORAGE.md). | `memory` for both - gone when the process ends; `journal_floor` 65536 |
| `seed` | Files of demo data - preconfigured components, personalities and session templates, and the sections of the plugins, `skills` among them - put into the stores at startup, only what they do not have yet, by id. A section of a seed file that no plugin claims is an error. | none |
| `secrets` | The chain of secret providers, each a [backend](#backends), tried in order: `type: file` with a `path` (a flat JSON key/value file), `type: env`. See [SECURITY.md](SECURITY.md#secrets). | `local/secrets.json`, then the environment |
| `users` | The chain of user providers, each a [backend](#backends). The only one so far is `type: accept_any`, a development stand-in that accepts any username and password; it gives the users named under `roles` their roles and everybody else `default_roles`, by the roles' names. See [SECURITY.md](SECURITY.md#users). | none - nobody can log in |
| `auth` | How the REST server keeps a browser logged in: `jwt_secret` (the name of the secret holding the signing key), `token_lifetime_secs`, `max_session_secs`, `secure_cookie`. See [REST-SERVER.md](REST-SERVER.md#authentication). | `CAELUMORA_JWT_SECRET`, 15 minutes, 12 hours, `true` |
| `security` | The access policy: `type: ownership`, or `type: allow_all` for tests and single-user development - it allows every action, but what may flow where still follows `compartments` and `roles`. See [SECURITY.md](SECURITY.md). | `ownership` |
| `roles` | The roles, each with an `id` - what is stored -, a `name` - what is written by hand - and the `clearances` it grants, a `level` and at most one `compartment`. Roles given by a user provider or named by a seed, and compartments named here, must be configured; the user provider's are checked when it is made. See [CONFIDENTIALITY.md](CONFIDENTIALITY.md#clearances). | none - clearances are not checked |
| `compartments` | The compartments a classification may name, each with a `name` and the compartments it `reads`; the read relation is closed transitively. See [CONFIDENTIALITY.md](CONFIDENTIALITY.md). | none - every classification is general |
| `server` | Where the REST server listens (`address`). | `127.0.0.1:3000` - this machine only, as long as `accept_any` may be enabled |
| `logging` | The log `format` (`text` or `json`) and an optional `file`. What is logged is selected with `RUST_LOG`. See [CODING-GUIDELINES.md](CODING-GUIDELINES.md#logging). | `text`; stderr (REST server) or `local/tui.log` (TUI) |
| `commands` | The user commands every session has, as a list of components; a session template can add its own. An empty list switches them off. See [COMMANDS.md](COMMANDS.md). | `help_command`, `wakeup_command` and `nudge_command` |
| `approvals` | What the system restricts of tool calls everywhere: `rules`, each with a `matcher` (`tool`, `*` for every one, and optionally `arguments`, a regular expression per argument), a `decision` (`ask` or `deny` - the system never allows) and optionally `expires_at`. See [APPROVALS.md](APPROVALS.md#rules-of-a-place). | none |
| `workspaces` | Where the files of workspaces lie: `max_file_bytes`, the largest file a person may put into one, and `storage`, the storages by name, each a [backend](#backends) - see [Workspace storages](#workspace-storages). A workspace names its storage when it is made. | 50 MiB; no storage - no workspace can be made |
| `artifacts` | The section of the plugin `caelumora-artifacts` (issue 328): where the content of published artifacts lies: `max_bytes`, the largest version; `storage`, the storages by name, each a [backend](#backends) - so far `type: local`, a directory per artifact under `root`, with `destination` (the most an artifact there may be classified; no bound unless given); and `publish_to`, the storage new artifacts are made in, needed where there are several. An artifact's later versions go where it was made. See [STORAGE.md](STORAGE.md). | 50 MiB; no storage - nothing can be published |
| `component_defaults` | Components that replace the default of a facet of which an agent or a channel has exactly one (`LLMClient`, `ContextProvider`; `Moderator`, `ResponderPolicy`), for every personality or channel that does not configure its own - except the `ContextProvider` of an external personality, whose default is its own. One component per such facet. See [COMPONENTS.md](COMPONENTS.md#facets-of-which-there-is-exactly-one). | the built-in defaults, and the program's model ([The model](#the-model)) |

The approval rules and the compartments are checked when the configuration is read; the backends,
`component_defaults` and the sections of the plugins when the runtime is built.

### Workspace storages

Every storage takes `max_run_secs` (3600), how long a command in one of its workspaces may run
before it is killed with what it started, and `destination`, the most a workspace there may be
classified (see [WORKSPACE.md](WORKSPACE.md#storages), which also says how far each provider keeps
its workspaces apart).

| `type` | Workspaces | Parameters |
|---|---|---|
| `local` | a directory per workspace under `root` on Caelumora's host | `shell` (default `false`); `destination` public by default where `shell` is on, else none |
| `open_terminal_shared` | every workspace in one Open Terminal instance started outside Caelumora (see `examples/docker-compose.yml`) | `endpoint` (as a component's, see [COMPONENTS.md](COMPONENTS.md#remote-endpoints); timeout 10 s by default), `mount` (an absolute path in the instance); `destination` public by default |
| `open_terminal_docker` | a volume and an Open Terminal container per workspace, started by Caelumora | `image` (pin it: the API is 0.x), `network` (`bridge` by default: the Docker network the containers run in and are reached at - it reaches what the agents are meant to reach, not Caelumora's own API), `idle_stop_secs` (900); `destination` public by default |

**`shell: true` runs what an agent's model writes on Caelumora's host, as its user, with nothing
between but the directory, approval and the rules of a place - for development only.**
The two Open Terminal storages are the plugin `caelumora-open-terminal`, which `caelumora-app` and
`caelumora-tui` list; `open_terminal_docker` needs its feature `docker` (on by default).

**`open_terminal_docker` needs the Docker daemon of Caelumora's environment, whose socket is root on
the host.**

## The model

The language model an agent talks to is a component: the `LLMClient` facet, of which an agent has
exactly one. A personality may configure its own; every other agent gets the one
`component_defaults` names, or else the default model of the program: `ollama_client` with its
defaults in `caelumora-app` and `caelumora-tui`, which list its plugin `caelumora-ollama` and name it
(issue 329) - the core names no model. There is no separate section for it, and no environment
variable.

```yaml
component_defaults:
  - id: model
    kind: ollama_client
    params:
      endpoint:
        url: "http://127.0.0.1:11434"
      model: "gemma4:26b"
      think: true
      reasoning_history: current_turn
      system_messages: only_at_start
      sender_format: prefix
      sender_style: handle
      context_size: 65536
```

| Parameter of `ollama_client` | Meaning | Default |
|---|---|---|
| `endpoint` | The Ollama server: its `url`, and its `timeout_secs` - how long a request may take to be answered, a streamed one until its stream begins. Ollama is reached without authentication: an `auth` is refused (see [COMPONENTS.md](COMPONENTS.md#remote-endpoints)) | `url: http://127.0.0.1:11434`, no timeout |
| `model` | The model | `gemma4:26b` |
| `think` | Whether the model is asked to reason; left out, Ollama's default for the model applies | left out |
| `reasoning_history` | Which earlier reasoning the model is given back: `never`, `current_turn` or `always` (see [AGENT-TURN.md](AGENT-TURN.md#reasoning-given-back)) | `current_turn` |
| `system_messages` | Where the model keeps system messages: `only_at_start` - any later one is given as a user message - or `anywhere` (see [AGENT-TURN.md](AGENT-TURN.md#system-messages)) | `only_at_start` |
| `sender_format` | How the model is told who wrote a message of another participant: `none`, `prefix` or `xml` (see [AGENT-TURN.md](AGENT-TURN.md#who-said-what)) | `prefix` |
| `sender_style` | By which name a sender is marked: `handle` or `name` | `handle` |
| `context_size` | How many tokens the model's context holds, sent to Ollama as `num_ctx`; left out, Ollama's default applies - which may be far below what the model was trained for, and Ollama cuts a longer prompt off without saying so | left out |

An unknown parameter is refused, and the agent does not start. Which values suit which model, as
far as measured, is in [LLM-MODELS.md](LLM-MODELS.md).

Before the first call to a model, the client reads the model's control tokens from the server and
keeps them from being produced by any text it sends; if they cannot be read, the call fails
(`llm::control_tokens`).

Every message the model produces carries the size of the context it was produced in: the configured
`context_size`, or else the size Ollama has the model loaded with, which may be unknown. Ollama
counts in a prompt's tokens all of the prompt, also what it kept from the previous call, so prompt
and answer against the context size say how full the context was.
