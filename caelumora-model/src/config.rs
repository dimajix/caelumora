use crate::command::default_commands;
use crate::component::ComponentConfig;
use crate::secret::SecretManager;
use crate::secret_impl::env::EnvSecretProvider;
use crate::secret_impl::file::FileSecretProvider;
use crate::security::{DummySecurityManager, OwnershipSecurityManager, SecurityManager};
use crate::store::{EntityStore, StateStore, StoreError};
use crate::store_impl::json::JsonFileStore;
use crate::store_impl::memory::MemoryStore;
use crate::user_provider::{UserManager, UserProvider};
use crate::user_provider_impl::accept_any::AcceptAnyUserProvider;
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
}

pub type ConfigResult<T> = Result<T, ConfigError>;

/// Where a store keeps its data.
#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersistenceBackend {
    /// In memory, gone when the process ends. The default.
    #[default]
    Memory,
    /// One JSON file per document below `path` (`JsonFileStore`).
    Json { path: PathBuf },
}

impl PersistenceBackend {
    /// Opens the store this backend describes. Both kinds of store serve
    /// as entity and as state store; the caller picks the face it needs.
    fn open(&self, security_manager: Arc<dyn SecurityManager>) -> ConfigResult<Arc<dyn Store>> {
        Ok(match self {
            Self::Memory => Arc::new(MemoryStore::with_security_manager(security_manager)),
            Self::Json { path } => Arc::new(JsonFileStore::new(path, security_manager)?),
        })
    }
}

/// What `PersistenceBackend::open` returns: a store with both faces, so
/// that one match serves `create_entity_store` and `create_state_store`.
trait Store: EntityStore + StateStore {}
impl<T: EntityStore + StateStore> Store for T {}

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
    /// `AcceptAnyUserProvider`). Never part of the default chain.
    AcceptAny,
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
    /// The user commands every session has (issue 28), as `CommandProvider`
    /// components; a session template can add its own. Defaults to `/help`
    /// and `/wakeup`; an empty `component_nodes` list switches them off.
    pub commands: ComponentConfig,
    /// The access policy; `ownership` unless configured otherwise.
    pub security: SecurityConfig,
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
            security: SecurityConfig::default(),
        }
    }
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
        // By default the parser quotes the offending lines of the input in its
        // error, and a configuration may hold credentials. The location alone
        // is enough to find the mistake.
        let mut options = serde_saphyr::Options::default();
        options.with_snippet = false;
        Ok(serde_saphyr::from_str_with_options(yaml, options)?)
    }

    /// The one `SecurityManager` of the process, as configured.
    pub fn create_security_manager(&self) -> Arc<dyn SecurityManager> {
        match self.security {
            SecurityConfig::Ownership => Arc::new(OwnershipSecurityManager::new()),
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
                UserProviderConfig::AcceptAny => {
                    tracing::warn!("User provider 'accept_any' is enabled: ANY username and password will be accepted. \
                        Development only - never expose this server to a network.");
                    Arc::new(AcceptAnyUserProvider::new(entity_store.clone())) as Arc<dyn UserProvider>
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
        assert_eq!(config.persistence.entity_store, PersistenceBackend::Json { path: PathBuf::from("local/entity_store") });
        assert_eq!(config.persistence.state_store, PersistenceBackend::Memory);
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
        assert!(matches!(config.users.providers[0], UserProviderConfig::AcceptAny));
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

        let config = AppConfig::from_yaml("commands:\n  component_nodes: []\n").unwrap();
        assert!(config.commands.component_nodes.is_empty(), "an explicit empty list means no global commands");
    }

    #[test]
    fn the_security_policy_defaults_to_ownership() {
        assert_eq!(AppConfig::default().security, SecurityConfig::Ownership);
        assert_eq!(AppConfig::from_yaml("persistence: {}").unwrap().security, SecurityConfig::Ownership);
        assert_eq!(AppConfig::from_yaml("security:\n  type: allow_all\n").unwrap().security, SecurityConfig::AllowAll);
    }
}
