use crate::approval::ApprovalRule;
use crate::command::default_commands;
use crate::confidentiality::{Classification, Compartments, ConfidentialityRules, Roles};
use std::collections::BTreeMap;
use crate::component::ComponentConfig;
use crate::remote::RemoteEndpoint;
use crate::secret::SecretManager;
use crate::secret::env::EnvSecretProvider;
use crate::secret::file::FileSecretProvider;
use crate::security::{DummySecurityManager, OwnershipSecurityManager, SecurityManager};
use crate::store::{ComponentStore, EntityStore, StateStore, StoreError};
use crate::store::json::JsonFileStore;
use crate::store::memory::MemoryStore;
use crate::user_provider::{UserManager, UserProvider};
use crate::user_provider::accept_any::AcceptAnyUserProvider;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] serde_saphyr::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Sections that do not fit together - a role naming a compartment that
    /// is not configured, say.
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

pub type ConfigResult<T> = Result<T, ConfigError>;

/// Where a store keeps its data.
#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersistenceBackend {
    /// In memory, gone when the process ends. The default.
    #[default]
    Memory,
    /// One JSON file per document below `path` (`JsonFileStore`); a state's
    /// journal is folded into its snapshot once it is `journal_floor` bytes
    /// large and larger than the snapshot (issue 193), 64 KiB unless named.
    Json {
        path: PathBuf,
        #[serde(default)]
        journal_floor: Option<u64>,
    },
}

impl PersistenceBackend {
    /// Opens the store this backend describes. Both kinds of store serve
    /// as entity and as state store; the caller picks the face it needs.
    fn open(&self, security_manager: Arc<dyn SecurityManager>) -> ConfigResult<Arc<dyn Store>> {
        Ok(match self {
            Self::Memory => Arc::new(MemoryStore::with_security_manager(security_manager)),
            Self::Json { path, journal_floor } => {
                let store = JsonFileStore::new(path, security_manager)?;
                Arc::new(match journal_floor {
                    Some(bytes) => store.with_journal_floor(*bytes),
                    None => store,
                })
            }
        })
    }
}

/// What `PersistenceBackend::open` returns: a store with every face, so
/// that one match serves `create_entity_store`, `create_component_store`
/// and `create_state_store`.
trait Store: EntityStore + ComponentStore + StateStore {}
impl<T: EntityStore + ComponentStore + StateStore> Store for T {}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct PersistenceConfig {
    pub entity_store: PersistenceBackend,
    pub state_store: PersistenceBackend,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretProviderConfig {
    File { path: PathBuf },
    Env,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct SecretsConfig {
    pub providers: Vec<SecretProviderConfig>,
}

/// Without a `secrets` section, secrets come from `local/secrets.json` and
/// then from the environment.
impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            providers: vec![
                SecretProviderConfig::File { path: PathBuf::from("local/secrets.json") },
                SecretProviderConfig::Env,
            ],
        }
    }
}

/// Which policy decides what may be done to what (issue 11).
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecurityConfig {
    /// Owners do everything with their entities, contributors read and use
    /// them, what belongs to the system everybody uses
    /// (`OwnershipSecurityManager`). The default: nothing is open unless a
    /// deployment says so.
    #[default]
    Ownership,
    /// Everything is allowed to everybody (`DummySecurityManager`) - for
    /// tests and single-user development setups only.
    AllowAll,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UserProviderConfig {
    /// Development only: accepts any username and password (see
    /// `AcceptAnyUserProvider`). Never part of the default chain. Gives the
    /// users named in `roles` their roles, and everybody else
    /// `default_roles` (issue 142).
    AcceptAny {
        #[serde(default)]
        roles: BTreeMap<String, Vec<String>>,
        #[serde(default)]
        default_roles: Vec<String>,
    },
}

/// Without a `users` section the chain is empty and nobody can
/// authenticate: there is no default provider, because the only one that
/// needs no setup accepts everybody.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct UsersConfig {
    pub providers: Vec<UserProviderConfig>,
}

/// How the REST server keeps a browser logged in: a signed JWT in an
/// `HttpOnly` cookie, renewed while in use (issue 10).
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    /// Name of the secret holding the JWT signing key, resolved through the
    /// secret chain. Also used as the token's `kid`, so rotating the key means
    /// pointing this at a new secret. If the secret does not exist, the server
    /// generates a random key at startup - which logs everybody out on every
    /// restart.
    pub jwt_secret: String,
    /// Lifetime of a token in seconds. A token is renewed once it is past half
    /// its lifetime; a session without requests for this long ends.
    pub token_lifetime_secs: u64,
    /// Absolute limit of a login in seconds, however active the user is.
    pub max_session_secs: u64,
    /// Whether the session cookie carries `Secure`. Browsers accept `Secure`
    /// cookies on `http://localhost`, so this only needs turning off for plain
    /// HTTP on another host - which should not happen outside a test setup.
    pub secure_cookie: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: "CAELUMORA_JWT_SECRET".to_string(),
            token_lifetime_secs: 15 * 60,
            max_session_secs: 12 * 60 * 60,
            secure_cookie: true,
        }
    }
}

/// Where the REST server listens (issue 106).
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Address and port, `127.0.0.1:3000` unless configured: reachable from
    /// this machine only, as long as `accept_any` may be enabled.
    pub address: std::net::SocketAddr,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { address: std::net::SocketAddr::from(([127, 0, 0, 1], 3000)) }
    }
}

/// Format of log output.
#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// Human-readable lines.
    #[default]
    Text,
    /// One JSON object per line, for log collectors.
    Json,
}

/// Where and how a binary writes its log (issue 01). Which records are
/// written is controlled by `RUST_LOG`, not here.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub format: LogFormat,
    /// Log file to append to. Without one, each binary uses its own default:
    /// the REST server writes to stderr, the TUI to `local/tui.log` so the
    /// log does not interfere with its prompt.
    pub file: Option<PathBuf>,
}

/// The configuration of a binary. Every section may be left out and then
/// takes its default; a key the configuration does not know is an error
/// rather than ignored, so that a misspelt section doesn't silently fall
/// back to its default.
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub persistence: PersistenceConfig,
    pub secrets: SecretsConfig,
    pub users: UsersConfig,
    pub auth: AuthConfig,
    pub logging: LoggingConfig,
    /// The user commands every session has (issue 28), as `ChannelCommandProvider`
    /// components; a session template can add its own. Defaults to `/help`
    /// and `/wakeup`; an empty list switches them off.
    pub commands: ComponentConfig,
    /// Components that replace the built-in default of a facet of which an
    /// agent has exactly one (issue 99) - the model it talks to (issue 130)
    /// or how it assembles the model's context, say - for every personality
    /// that does not configure its own.
    /// Empty unless configured: the built-in defaults apply.
    pub component_defaults: ComponentConfig,
    /// The access policy; `ownership` unless configured otherwise.
    pub security: SecurityConfig,
    /// The compartments of the classification (issue 133) and which may
    /// read which. Checked when the configuration is read. None unless
    /// configured: every classification is general.
    pub compartments: Compartments,
    /// The roles and the clearances each grants (issues 133, 142). Checked
    /// when the configuration is read. `None` unless configured: then
    /// clearances are not checked at all.
    pub roles: Option<Roles>,
    /// Files of demo data - personalities and session templates - put into
    /// the entity store at startup (issue 105, see `examples/seed.yaml`).
    /// None unless configured.
    pub seed: Vec<PathBuf>,
    /// Where the REST server listens.
    pub server: ServerConfig,
    /// What the system restricts of tool calls everywhere (issue 13).
    /// Checked when the configuration is read. None unless configured.
    pub approvals: ApprovalsConfig,
    /// Where the files of workspaces lie (issue 253). No storage unless
    /// configured: then no workspace can be made.
    pub workspaces: WorkspacesConfig,
    /// Where the content of published artifacts lies (issue 271). No
    /// storage unless configured: then no artifact can be published.
    pub artifacts: ArtifactsConfig,
}

/// The storages workspaces are made in (issue 253), each by the name a
/// workspace refers to it with and served by its provider, and how large a
/// file a person may put into one.
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspacesConfig {
    pub max_file_bytes: u64,
    pub storage: BTreeMap<String, StorageConfig>,
}

/// The largest file a person may put into a workspace unless configured
/// otherwise: 50 MiB.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

impl Default for WorkspacesConfig {
    fn default() -> Self {
        Self { max_file_bytes: DEFAULT_MAX_FILE_BYTES, storage: BTreeMap::new() }
    }
}

/// A storage of workspaces, by the provider that makes and reaches the
/// place of each (issue 251, part A).
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum StorageConfig {
    /// A directory per workspace under `root`, on Caelumora's host, read
    /// and written by Caelumora itself. With `shell`, agents run commands
    /// there - on Caelumora's host, as its user: for development and tests,
    /// never where the host is to be protected.
    Local {
        root: PathBuf,
        #[serde(default)]
        shell: bool,
        /// What may be put into this storage (issue 251): the classification
        /// a workspace in it may have at most. Without it, `public` where it
        /// has a shell - a command may send anything anywhere - and no bound
        /// where it has none, since nothing then leaves the process.
        #[serde(default)]
        destination: Option<Classification>,
    },
    /// One Open Terminal instance, started outside Caelumora, at
    /// `endpoint`, that has the storage at `mount`: a workspace is the
    /// directory `<mount>/<id>` there. Every workspace of the storage lies
    /// in the one instance, and a command in one reaches every other.
    OpenTerminalShared {
        endpoint: RemoteEndpoint,
        /// Where the instance has the storage: absolute, in the instance.
        mount: String,
        /// What may be put into this storage; `public` unless given - a
        /// command may send anything anywhere.
        #[serde(default)]
        destination: Option<Classification>,
    },
    /// A volume and an Open Terminal container per workspace, started by
    /// Caelumora over the Docker daemon its environment names - which is
    /// root on the host - on first use and stopped after `idle_stop_secs`
    /// unused. A workspace's commands are kept to its container.
    OpenTerminalDocker {
        /// Pinned: Open Terminal's API is 0.x.
        image: String,
        /// The Docker network the containers are started in, which reaches
        /// what the agents are meant to reach; Caelumora reaches them at
        /// their address there.
        #[serde(default = "default_docker_network")]
        network: String,
        #[serde(default = "default_idle_stop_secs")]
        idle_stop_secs: u64,
        /// What may be put into this storage; `public` unless given - a
        /// command reaches whatever its network does.
        #[serde(default)]
        destination: Option<Classification>,
    },
}

/// The storages artifacts are kept in (issue 271), each by the name an
/// artifact refers to it with, which of them new artifacts go to, and how
/// large a version may be.
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct ArtifactsConfig {
    pub max_bytes: u64,
    /// The storage new artifacts are made in; needed where there are
    /// several, the one there is where there is one.
    pub publish_to: Option<String>,
    pub storage: BTreeMap<String, ArtifactStorageConfig>,
}

impl Default for ArtifactsConfig {
    fn default() -> Self {
        Self { max_bytes: DEFAULT_MAX_FILE_BYTES, publish_to: None, storage: BTreeMap::new() }
    }
}

impl ArtifactsConfig {
    /// The storage new artifacts are made in: `publish_to`, or the one
    /// configured; `None` where there is none.
    pub fn publish_to(&self) -> Option<&str> {
        match (&self.publish_to, self.storage.len()) {
            (Some(name), _) => Some(name.as_str()),
            (None, 1) => self.storage.keys().next().map(String::as_str),
            (None, _) => None,
        }
    }

    /// Whether it fits together: `publish_to` names a configured storage,
    /// and is given where there are several.
    ///
    /// # Errors
    /// What does not fit.
    pub fn check(&self) -> Result<(), String> {
        match &self.publish_to {
            Some(name) if !self.storage.contains_key(name) => Err(format!("publish_to names '{name}', which is no storage of artifacts")),
            None if self.storage.len() > 1 => Err("publish_to is needed where there are several storages of artifacts".to_string()),
            _ => Ok(()),
        }
    }
}

/// A storage of artifacts, by its provider (issue 271); object storage is
/// issue 272.
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactStorageConfig {
    /// A directory per artifact under `root`, a file per version, on
    /// Caelumora's host.
    Local {
        root: PathBuf,
        /// What may be put into this storage: the classification an
        /// artifact in it may have at most; no bound unless given, since
        /// nothing then leaves the host.
        #[serde(default)]
        destination: Option<Classification>,
    },
}

fn default_docker_network() -> String {
    "bridge".to_string()
}

fn default_idle_stop_secs() -> u64 {
    900
}

impl StorageConfig {
    /// Whether its fields fit: a `mount` is absolute and stays where it
    /// says.
    ///
    /// # Errors
    /// What does not fit.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Self::Local { .. } => Ok(()),
            Self::OpenTerminalShared { mount, .. } => {
                if !mount.starts_with('/') || mount.contains('\0') || mount.split('/').any(|part| part == "..") {
                    return Err(format!("the mount '{mount}' is not an absolute path that stays where it says"));
                }
                Ok(())
            }
            Self::OpenTerminalDocker { image, network, idle_stop_secs, .. } => {
                if image.trim().is_empty() {
                    return Err("an image is needed".to_string());
                }
                if matches!(network.as_str(), "" | "host" | "none") || network.starts_with("container:") {
                    return Err(format!("the network '{network}' does not give a container an address of its own"));
                }
                if *idle_stop_secs == 0 {
                    return Err("idle_stop_secs must be more than 0".to_string());
                }
                Ok(())
            }
        }
    }
}

/// The system's rules on tool calls (issue 13): each asks or denies - the
/// system is a place, and a place only restricts.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalsConfig {
    pub rules: Vec<ApprovalRule>,
}

// Not derived: the commands default to the built-in ones, not to none.
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            persistence: PersistenceConfig::default(),
            secrets: SecretsConfig::default(),
            users: UsersConfig::default(),
            auth: AuthConfig::default(),
            logging: LoggingConfig::default(),
            commands: default_commands(),
            component_defaults: ComponentConfig::default(),
            security: SecurityConfig::default(),
            compartments: Compartments::default(),
            roles: None,
            seed: Vec::new(),
            server: ServerConfig::default(),
            approvals: ApprovalsConfig::default(),
            workspaces: WorkspacesConfig::default(),
            artifacts: ArtifactsConfig::default(),
        }
    }
}

/// Parses a document the way the configuration is parsed, for files a
/// configuration names (e.g. `seed`).
///
/// # Errors
/// `ConfigError::Parse` if `yaml` is not a `T`; the error names the line and
/// column, but quotes nothing of the input.
pub fn parse_yaml<T: serde::de::DeserializeOwned>(yaml: &str) -> ConfigResult<T> {
    // By default the parser quotes the offending lines of the input in its
    // error, and a configuration may hold credentials. The location alone
    // is enough to find the mistake.
    let mut options = serde_saphyr::Options::default();
    options.with_snippet = false;
    Ok(serde_saphyr::from_str_with_options(yaml, options)?)
}

impl AppConfig {
    /// Reads and parses the configuration file at `path`.
    ///
    /// # Errors
    /// `ConfigError::Io` if the file cannot be read, `ConfigError::Parse` if
    /// it is not a valid configuration.
    pub fn load(path: impl AsRef<Path>) -> ConfigResult<Self> {
        let content = fs::read_to_string(path)?;
        Self::from_yaml(&content)
    }

    /// Parses a configuration from YAML text.
    ///
    /// # Errors
    /// `ConfigError::Parse` if `yaml` is not a valid configuration; the error
    /// names the line and column.
    pub fn from_yaml(yaml: &str) -> ConfigResult<Self> {
        let config: Self = parse_yaml(yaml)?;
        config.check()?;
        Ok(config)
    }

    /// Nothing if the sections fit together: the roles grant only
    /// configured compartments, and the user providers give only
    /// configured roles - and none without a `roles` section.
    fn check(&self) -> ConfigResult<()> {
        let invalid = |e: crate::confidentiality::RoleError| ConfigError::Invalid(e.to_string());
        if let Some(roles) = &self.roles {
            roles.check(&self.compartments).map_err(invalid)?;
        }
        for (name, storage) in &self.workspaces.storage {
            storage.check().map_err(|why| ConfigError::Invalid(format!("the storage '{name}' of workspaces: {why}")))?;
        }
        self.artifacts.check().map_err(|why| ConfigError::Invalid(format!("artifacts: {why}")))?;
        for rule in &self.approvals.rules {
            rule.check_restricting().map_err(|e| ConfigError::Invalid(format!("an approval rule for `{}`: {e}", rule.matcher.tool)))?;
        }
        for provider in &self.users.providers {
            let UserProviderConfig::AcceptAny { roles: assigned, default_roles } = provider;
            for role in assigned.values().flatten().chain(default_roles) {
                match &self.roles {
                    Some(roles) => roles.check_role(role).map_err(invalid)?,
                    None => return Err(ConfigError::Invalid(format!("role '{role}' is given to users, but no roles are configured"))),
                }
            }
        }
        Ok(())
    }

    /// The one `SecurityManager` of the process, as configured, with the
    /// confidentiality rules of the configured compartments and roles where
    /// the policy applies them.
    pub fn create_security_manager(&self) -> Arc<dyn SecurityManager> {
        match self.security {
            SecurityConfig::Ownership => {
                let rules = ConfidentialityRules::new(self.compartments.clone());
                Arc::new(OwnershipSecurityManager::with_rules(match &self.roles {
                    Some(roles) => rules.with_roles(roles.clone()),
                    None => rules,
                }))
            }
            SecurityConfig::AllowAll => {
                tracing::warn!("Security policy 'allow_all' is configured: every user may do everything. Development only.");
                Arc::new(DummySecurityManager::new())
            }
        }
    }

    /// The entity store, as configured under `persistence.entity_store`.
    ///
    /// # Errors
    /// `ConfigError::Store` if the store cannot be opened.
    pub fn create_entity_store(&self, security_manager: Arc<dyn SecurityManager>) -> ConfigResult<Arc<dyn EntityStore>> {
        Ok(self.persistence.entity_store.open(security_manager)?)
    }

    /// The component store: kept where the entity store is, under
    /// `persistence.entity_store`, as the preconfigured components were
    /// before they had a store of their own.
    ///
    /// # Errors
    /// `ConfigError::Store` if the store cannot be opened.
    pub fn create_component_store(&self, security_manager: Arc<dyn SecurityManager>) -> ConfigResult<Arc<dyn ComponentStore>> {
        Ok(self.persistence.entity_store.open(security_manager)?)
    }

    /// The state store, as configured under `persistence.state_store`.
    ///
    /// # Errors
    /// `ConfigError::Store` if the store cannot be opened.
    pub fn create_state_store(&self, security_manager: Arc<dyn SecurityManager>) -> ConfigResult<Arc<dyn StateStore>> {
        Ok(self.persistence.state_store.open(security_manager)?)
    }

    /// The chain of secret providers from the `secrets` section.
    pub fn create_secret_manager(&self, security_manager: Arc<dyn SecurityManager>) -> ConfigResult<Arc<SecretManager>> {
        let providers = self.secrets.providers.iter()
            .map(|config| match config {
                SecretProviderConfig::File { path } => Arc::new(FileSecretProvider::new(path.clone())) as Arc<dyn crate::secret::SecretProvider>,
                SecretProviderConfig::Env => Arc::new(EnvSecretProvider::new()) as Arc<dyn crate::secret::SecretProvider>,
            })
            .collect();

        Ok(Arc::new(SecretManager::new(providers, security_manager)))
    }

    /// The chain of user providers from the `users` section.
    pub fn create_user_manager(&self, entity_store: Arc<dyn EntityStore>) -> ConfigResult<Arc<UserManager>> {
        let providers = self.users.providers.iter()
            .map(|config| match config {
                UserProviderConfig::AcceptAny { roles, default_roles } => {
                    tracing::warn!("User provider 'accept_any' is enabled: ANY username and password will be accepted. \
                        Development only - never expose this server to a network.");
                    Arc::new(AcceptAnyUserProvider::with_roles(Arc::clone(&entity_store), roles.clone(), default_roles.clone())) as Arc<dyn UserProvider>
                }
            })
            .collect();

        Ok(Arc::new(UserManager::new(providers)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_backend_is_named_by_its_type() {
        let yaml = r#"
persistence:
  entity_store:
    type: json
    path: "local/entity_store"
  state_store:
    type: memory
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.persistence.entity_store, PersistenceBackend::Json { path: PathBuf::from("local/entity_store"), journal_floor: None });
        assert_eq!(config.persistence.state_store, PersistenceBackend::Memory);

        let with_floor = AppConfig::from_yaml("persistence:\n  state_store:\n    type: json\n    path: \"s\"\n    journal_floor: 1024\n").unwrap();
        assert_eq!(with_floor.persistence.state_store, PersistenceBackend::Json { path: PathBuf::from("s"), journal_floor: Some(1024) });
    }

    #[test]
    fn a_missing_section_takes_its_default() {
        for config in [AppConfig::default(), AppConfig::from_yaml("{}").unwrap(), AppConfig::from_yaml("persistence: {}").unwrap()] {
            assert_eq!(config.persistence.entity_store, PersistenceBackend::Memory);
            assert_eq!(config.persistence.state_store, PersistenceBackend::Memory);
            assert_eq!(config.secrets.providers, SecretsConfig::default().providers);
            assert!(config.users.providers.is_empty(), "no user provider unless configured");
        }
    }

    #[test]
    fn artifacts_are_kept_in_the_storage_publish_to_names_or_the_one_there_is() {
        let one = AppConfig::from_yaml("artifacts:\n  storage:\n    files:\n      provider: local\n      root: local/artifacts\n").expect("one local storage");
        assert_eq!(one.artifacts.publish_to(), Some("files"));
        assert_eq!(one.artifacts.max_bytes, DEFAULT_MAX_FILE_BYTES);
        let none = AppConfig::from_yaml("{}").unwrap();
        assert_eq!(none.artifacts.publish_to(), None, "none configured");
        let several = "artifacts:\n  storage:\n    a:\n      provider: local\n      root: a\n    b:\n      provider: local\n      root: b\n";
        assert!(AppConfig::from_yaml(several).is_err(), "which one, among several, must be said");
        let chosen = AppConfig::from_yaml(&format!("{several}  publish_to: b\n")).expect("chosen");
        assert_eq!(chosen.artifacts.publish_to(), Some("b"));
        assert!(AppConfig::from_yaml(&format!("{several}  publish_to: c\n")).is_err(), "not one that is not there");
    }

    #[test]
    fn workspaces_are_made_in_the_storages_configured() {
        let config = AppConfig::from_yaml("workspaces:\n  max_file_bytes: 1024\n  storage:\n    main:\n      provider: local\n      root: local/workspaces\n").expect("a local storage");
        assert_eq!(config.workspaces.max_file_bytes, 1024);
        assert_eq!(config.workspaces.storage.get("main"), Some(&StorageConfig::Local { root: PathBuf::from("local/workspaces"), shell: false, destination: None }));
        let shell = AppConfig::from_yaml("workspaces:\n  storage:\n    dev:\n      provider: local\n      root: x\n      shell: true\n      destination: { level: internal }\n").expect("a local storage with a shell");
        assert_eq!(shell.workspaces.storage.get("dev"), Some(&StorageConfig::Local { root: PathBuf::from("x"), shell: true, destination: Some(Classification::general(crate::confidentiality::Confidentiality::Internal)) }));
        let none = AppConfig::from_yaml("{}\n").expect("nothing configured");
        assert!(none.workspaces.storage.is_empty(), "no storage unless configured");
        assert_eq!(none.workspaces.max_file_bytes, DEFAULT_MAX_FILE_BYTES);
        for yaml in ["workspaces:\n  storage:\n    main:\n      provider: local\n      root: x\n      rot: y\n", "workspaces:\n  storage:\n    main:\n      provider: nfs\n"] {
            assert!(AppConfig::from_yaml(yaml).is_err(), "{yaml:?}");
        }
    }

    #[test]
    fn an_open_terminal_storage_names_its_instance_and_where_it_has_the_storage() {
        let yaml = "workspaces:\n  storage:\n    sandbox:\n      provider: open_terminal_shared\n      endpoint: { url: \"http://open-terminal:8000\", auth: { type: bearer, secret: OPEN_TERMINAL_API_KEY } }\n      mount: /workspaces\n";
        let config = AppConfig::from_yaml(yaml).expect("a shared Open Terminal");
        let Some(StorageConfig::OpenTerminalShared { endpoint, mount, destination }) = config.workspaces.storage.get("sandbox") else { panic!("{:?}", config.workspaces.storage) };
        assert_eq!((endpoint.url.as_str(), mount.as_str(), destination), ("http://open-terminal:8000", "/workspaces", &None));
        for mount in ["workspaces", "/a/../b"] {
            let refused = AppConfig::from_yaml(&yaml.replace("/workspaces", mount)).map(|_| ()).unwrap_err().to_string();
            assert!(refused.contains("sandbox") && refused.contains("mount"), "{refused}");
        }
        assert!(AppConfig::from_yaml(&yaml.replace("      mount: /workspaces\n", "")).is_err(), "a mount is needed");
    }

    #[test]
    fn a_docker_storage_names_its_image_and_takes_the_rest_by_default() {
        let yaml = "workspaces:\n  storage:\n    isolated:\n      provider: open_terminal_docker\n      image: ghcr.io/open-webui/open-terminal:0.14.0\n";
        let config = AppConfig::from_yaml(yaml).expect("a container per workspace");
        let Some(StorageConfig::OpenTerminalDocker { image, network, idle_stop_secs, destination }) = config.workspaces.storage.get("isolated") else { panic!() };
        assert_eq!((image.as_str(), network.as_str(), *idle_stop_secs, destination), ("ghcr.io/open-webui/open-terminal:0.14.0", "bridge", 900, &None));
        for (wrong, why) in [("      network: host\n", "network"), ("      idle_stop_secs: 0\n", "idle_stop_secs")] {
            let refused = AppConfig::from_yaml(&format!("{yaml}{wrong}")).map(|_| ()).unwrap_err().to_string();
            assert!(refused.contains("isolated") && refused.contains(why), "{refused}");
        }
        assert!(AppConfig::from_yaml(&yaml.replace("ghcr.io/open-webui/open-terminal:0.14.0", "\" \"")).is_err(), "an image is needed");
    }

    #[test]
    fn an_unknown_key_is_an_error_naming_it() {
        for yaml in [
            "persistence:\n  entity_stor:\n    type: memory\n",
            "persistence:\n  entity_store:\n    type: json\n    pth: local\n",
            "secret:\n  providers: []\n",
            "auth:\n  token_lifetime: 60\n",
        ] {
            let message = AppConfig::from_yaml(yaml).expect_err(yaml).to_string();
            assert!(message.contains("unknown field"), "{yaml:?} gave: {message}");
        }
        let message = AppConfig::from_yaml("persistence:\n  state_store:\n    type: file\n").expect_err("unknown backend").to_string();
        assert!(message.contains("unknown variant"), "gave: {message}");
    }

    #[test]
    fn the_example_config_loads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/config.yaml");
        let config = AppConfig::load(path).expect("examples/config.yaml is a valid configuration");
        assert!(matches!(config.persistence.entity_store, PersistenceBackend::Json { .. }));
        assert_eq!(config.seed, [PathBuf::from("examples/seed.yaml")], "the example seeds the demo data");
    }

    #[test]
    fn test_load_config_secrets() {
        let yaml = r#"
secrets:
  providers:
    - type: file
      path: "local/secrets.json"
    - type: env
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.secrets.providers, [
            SecretProviderConfig::File { path: PathBuf::from("local/secrets.json") },
            SecretProviderConfig::Env,
        ]);
    }

    #[test]
    fn test_load_config_users() {
        let yaml = r#"
users:
  providers:
    - type: accept_any
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.users.providers.len(), 1);
        assert!(matches!(config.users.providers[0], UserProviderConfig::AcceptAny { .. }));
    }

    #[test]
    fn test_load_config_logging() {
        let config = AppConfig::from_yaml("persistence: {}").unwrap();
        assert_eq!(config.logging.format, LogFormat::Text);
        assert!(config.logging.file.is_none());

        let yaml = r#"
logging:
  format: json
  file: "local/server.log"
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.logging.format, LogFormat::Json);
        assert_eq!(config.logging.file, Some(PathBuf::from("local/server.log")));
    }

    #[test]
    fn test_load_config_auth_defaults_and_overrides() {
        let config = AppConfig::from_yaml("persistence: {}").unwrap();
        assert_eq!(config.auth.token_lifetime_secs, 900);
        assert!(config.auth.secure_cookie);

        let yaml = r#"
auth:
  jwt_secret: "OTHER_KEY"
  token_lifetime_secs: 60
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.auth.jwt_secret, "OTHER_KEY");
        assert_eq!(config.auth.token_lifetime_secs, 60);
        assert_eq!(config.auth.max_session_secs, 12 * 60 * 60, "unset fields keep their defaults");
    }

    #[tokio::test]
    async fn test_create_user_manager_without_config_accepts_nobody() {
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        let manager = AppConfig::default().create_user_manager(store).unwrap();

        let result = manager.authenticate("user", &crate::user_provider::Credential::Password("x")).await;
        assert!(result.is_err(), "no provider must be enabled by default");
    }

    #[tokio::test]
    async fn test_create_user_manager_with_accept_any() {
        let yaml = r#"
users:
  providers:
    - type: accept_any
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        let manager = config.create_user_manager(store).unwrap();

        let user = manager.authenticate("alice", &crate::user_provider::Credential::Password("x")).await
            .expect("accept_any accepts everybody");
        assert_eq!(user.username, "alice");
    }

    #[tokio::test]
    async fn test_create_secret_manager_defaults_fall_through_to_env() {
        // With no `secrets` config, the default provider chain is
        // [File("local/secrets.json"), Env]. Since that file does not exist
        // in the test working directory, a lookup should fall through to
        // the env provider.
        std::env::set_var("CAELUMORA_TEST_CONFIG_SECRET", "value-from-env");
        let config = AppConfig::default();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let manager = config.create_secret_manager(security_manager).unwrap();
        let value = manager
            .resolve(&crate::security::SecurityContext::default(), "CAELUMORA_TEST_CONFIG_SECRET")
            .await
            .unwrap();
        assert_eq!(value, "value-from-env");
        std::env::remove_var("CAELUMORA_TEST_CONFIG_SECRET");
    }

    #[test]
    fn a_malformed_config_names_the_location_but_not_the_content() {
        // The marker is a value: keys are named in errors about unknown
        // fields, but what a key holds may be a credential.
        let yaml = "auth:\n  jwt_secret: [secret_value_xyz, unclosed\n";
        let err = AppConfig::from_yaml(yaml).expect_err("unclosed flow sequence must not parse");
        let message = err.to_string();
        assert!(message.contains("line"), "error should name the line: {message}");
        assert!(!message.contains("secret_value_xyz"), "error must not quote the input: {message}");
    }

    #[test]
    fn the_commands_default_to_the_builtin_ones_and_can_be_switched_off() {
        let expected = crate::command::default_commands().component_nodes.len();
        assert_eq!(AppConfig::default().commands.component_nodes.len(), expected);
        assert_eq!(AppConfig::from_yaml("persistence: {}").unwrap().commands.component_nodes.len(), expected);

        let config = AppConfig::from_yaml("commands: []\n").unwrap();
        assert!(config.commands.component_nodes.is_empty(), "an explicit empty list means no global commands");
    }

    #[test]
    fn the_server_listens_on_this_machine_unless_configured() {
        assert_eq!(AppConfig::default().server.address.to_string(), "127.0.0.1:3000");
        let config = AppConfig::from_yaml("server:\n  address: \"0.0.0.0:8080\"\n").unwrap();
        assert_eq!(config.server.address.to_string(), "0.0.0.0:8080");
        assert!(AppConfig::from_yaml("server:\n  address: nowhere\n").is_err(), "an address that is none");
    }

    #[test]
    fn compartments_are_checked_when_the_configuration_is_read() {
        use crate::confidentiality::Compartment;
        let config = AppConfig::from_yaml("compartments:\n  - name: hr\n    reads: [finance]\n  - name: finance\n").unwrap();
        assert!(config.compartments.may_read(&Compartment::new("hr"), &Compartment::new("finance")));
        assert_eq!(AppConfig::default().compartments.names().count(), 0, "none unless configured");
        assert!(AppConfig::from_yaml("compartments:\n  - name: hr\n    reads: [payroll]\n").is_err(), "reads what is not configured");
        assert!(AppConfig::from_yaml("compartments:\n  - name: hr\n    read: [finance]\n").is_err(), "a misspelt key");
    }

    #[test]
    fn approval_rules_are_checked_when_the_configuration_is_read() {
        let config = AppConfig::from_yaml("approvals:\n  rules:\n    - matcher: { tool: shell, arguments: { command: 'rm .*' } }\n      decision: deny\n    - matcher: { tool: '*' }\n      decision: ask\n").unwrap();
        assert_eq!(config.approvals.rules.len(), 2);
        assert!(AppConfig::default().approvals.rules.is_empty(), "none unless configured");
        assert!(matches!(AppConfig::from_yaml("approvals:\n  rules:\n    - matcher: { tool: shell }\n      decision: allow\n"), Err(ConfigError::Invalid(_))), "the system does not allow");
        assert!(matches!(AppConfig::from_yaml("approvals:\n  rules:\n    - matcher: { tool: shell, arguments: { command: '(' } }\n      decision: deny\n"), Err(ConfigError::Invalid(_))), "a pattern that cannot be matched");
        assert!(AppConfig::from_yaml("approvals:\n  rule: []\n").is_err(), "a misspelt key");
    }

    #[test]
    fn roles_are_checked_against_the_compartments_and_the_providers() {
        let compartments = "compartments:\n  - name: hr\n";
        let roles = "roles:\n  - name: hr\n    clearances:\n      - { level: confidential, compartment: hr }\n";
        let config = AppConfig::from_yaml(&format!("{compartments}{roles}users:\n  providers:\n    - type: accept_any\n      roles: {{ alice: [hr] }}\n")).unwrap();
        assert!(config.roles.is_some());
        assert!(AppConfig::default().roles.is_none(), "none unless configured: clearances are not checked");

        let typo = "roles:\n  - name: hr\n    clearances:\n      - { level: confidential, compartment: hrr }\n";
        assert!(matches!(AppConfig::from_yaml(&format!("{compartments}{typo}")), Err(ConfigError::Invalid(_))), "a compartment not configured");
        let unknown = format!("{compartments}{roles}users:\n  providers:\n    - type: accept_any\n      default_roles: [staff]\n");
        assert!(matches!(AppConfig::from_yaml(&unknown), Err(ConfigError::Invalid(_))), "a role not configured");
        let without = "users:\n  providers:\n    - type: accept_any\n      default_roles: [staff]\n";
        assert!(matches!(AppConfig::from_yaml(without), Err(ConfigError::Invalid(_))), "roles given, but none configured");
    }

    #[test]
    fn the_security_policy_defaults_to_ownership() {
        assert_eq!(AppConfig::default().security, SecurityConfig::Ownership);
        assert_eq!(AppConfig::from_yaml("persistence: {}").unwrap().security, SecurityConfig::Ownership);
        assert_eq!(AppConfig::from_yaml("security:\n  type: allow_all\n").unwrap().security, SecurityConfig::AllowAll);
    }
}
