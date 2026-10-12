# 324 — Every configured backend is a closed enum, so no backend can be added from outside the core

**Status:** open
**Area:** Code structure

## The problem

Several services of Caelumora are served by one of a choice of backends, named in the
configuration. Each choice is a closed `enum` of `caelumora-model/src/config.rs`, matched where the
service is made:

| Configuration | Matched in | Into |
|---|---|---|
| `StorageConfig` (`workspaces.storage`, tag `provider`) | `WorkspaceManager::new`, `StorageConfig::max_run`, `StorageConfig::check` | `TerminalProvider` (`pub(crate)`) |
| `ArtifactStorageConfig` (`artifacts.storage`, tag `provider`) | `ArtifactManager::new` | `ArtifactStorage` (`pub(crate)`) |
| `UserProviderConfig` (`users.providers`, tag `type`) | `AppConfig::create_user_manager` | `UserProvider` |
| `SecretProviderConfig` (`secrets.providers`, tag `type`) | `AppConfig::create_secret_manager` | `SecretProvider` |
| `PersistenceBackend` (`persistence.*`, tag `type`) | `PersistenceBackend::open` | `Store` |

A backend that is not one of the variants cannot be added without editing the core: the enum, the
`match` that makes it, and every further `match` over the variants (`max_run`, `check`). Issue
[323](323-kinds-enumerated-in-the-core.md) opens what the core enumerates of stored *kinds*; it
gives a plugin whole sections of the configuration of its own (cut 8), but no way to add a variant
to a section of the core's. The backends are the other axis, and they are not covered.

## Why it matters

- Open Terminal (issue [327](327-open-terminal-in-a-crate-of-its-own.md)) cannot move into a crate
  of its own, and with it `bollard`, while `StorageConfig` names it.
- Every backend still to come is of this shape and would bring its dependencies into the core: S3
  for artifacts ([272](272-artifacts-in-s3.md)), a database of users
  ([26](26-database-user-backend.md)), Postgres as entity and state store
  ([03](03-postgres-entity-store.md), [04](04-postgres-state-store.md)).
- The five are built five ways - two in the runtime's managers, three in the model's `AppConfig` -,
  and two tag the backend `provider`, three `type`. Neither difference has a reason.
- The model makes things here: `AppConfig::create_user_manager` and `create_secret_manager` build
  the providers, which by `ARCHITECTURE.md` is the runtime's business, not the model's.

## Considerations

### How a backend is named and made

- **Option A - keep the enums; a crate adds its variant to the core.** The status quo. Rejected:
  it is what is to be got rid of.
- **Option B - a registry of backends per service, generic over the service's trait.** A backend is
  configured as its name and its parameters (`{ provider: "open_terminal_docker", ... }`), kept as
  `serde_json::Value` until it is made; a factory registered under the name makes it, as the
  `ComponentRegistry` does for component kinds. One generic type serves all five, say
  `Backends<dyn TerminalProvider>`, filled by the composition root - the `RuntimeBuilder` of issue
  323, step 9 - before the services are made. The checks of a variant (`check`, `max_run`) move
  into the backend's own configuration type.
  *Benefit:* one mechanism for five services and every one to come; a backend is added by linking
  a crate. *Cost:* a configuration that names an unknown backend, or gives it wrong parameters, is
  refused when the runtime is built instead of when the file is parsed - still at start-up, with
  the same message; the traits `TerminalProvider` and `ArtifactStorage` and what their signatures
  need (`Terminal`, `Shell`, `WorkspacePath`, `CommandKey`, `ByteStream`, `TerminalError`, …)
  become public API.
- **Option C - backends are components.** The `ComponentRegistry` already maps a kind and its
  parameters to facets; a backend would be a component whose facet is the service's trait. No
  second registry - but components are built per container, for an agent or a channel, with a
  `ComponentContext` of that container, while a backend is built once for the process, from the
  configuration, before any container exists. It would bend the components to a case they are not
  made for.

**Recommendation:** B, built to look like the `ComponentRegistry` where the two do the same: the
name of the backend as a component's `kind`, its parameters read by `config_of_params`, the same
error for an unknown name or bad parameters. A deliberate difference from it - built once, no
container - is the reason it is not C.

### Order of construction

Secrets are needed to make other backends (an Open Terminal's endpoint, an MCP server's), and the
user and secret providers are made today before the runtime, from `AppConfig`. The registry has to
be filled before any of them is made, so the composition root registers every backend first and
then makes the services in the order of today. Moving `create_user_manager` and
`create_secret_manager` out of the model belongs to this issue.

### One tag

Whether the backend is named by `provider` or by `type` is decided here, for all five alike. Either
way, the configuration files of the sections that change their tag have to be changed by hand -
configuration, not stored data.

### Not here

- `SecurityConfig` (`Ownership`, `AllowAll`) is a choice of the same shape, but the policy is no
  backend of a service: it decides for all of them. Whether it is opened too is a question of its
  own.
- What each backend crate holds is its own issue (327; 272, 26, 03 and 04 when they are done).

## Done when

- The five configurations name their backend by one tag and keep its parameters for its factory;
  no `match` over the backends is left in the core.
- A crate registers a backend through the composition root, and the core compiles and passes its
  tests without the backends that are not its own.
- `docs/CONFIGURATION.md` says how a backend is named and that an unknown one is refused at
  start-up; `docs/ARCHITECTURE.md` says how a backend is added.

## History

- **2026-10-12** — Opened from the discussion of which subjects besides skills could move into
  crates of their own (issue 323). Open Terminal turned out to be blocked by `StorageConfig`, and
  the same closed shape was found in four further places.
