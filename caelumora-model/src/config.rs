use crate::approval::ApprovalRule;
use crate::command::default_commands;
use crate::confidentiality::{Compartments, ConfidentialityPolicy, ConfidentialityRules};
use crate::role::Roles;
use std::collections::BTreeMap;
use crate::component::ComponentConfig;
use crate::security::{AllowAllPolicy, Memberships, OwnershipPolicy, AccessPolicy};
use crate::store::{Persistence, StoreBackend, StoreError};
use crate::store::json::JsonFileStore;
use crate::store::memory::MemoryStore;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use serde_saphyr::Options;
use std::io::Error;
use std::net::SocketAddr;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] Error),
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
    /// The stores of the core's kinds and the components are opened at
    /// once, so that a store that cannot keep them is not opened at all.
    fn open(&self, access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>) -> ConfigResult<StoreBackend> {
        let persistence: Arc<dyn Persistence> = match self {
            Self::Memory => Arc::new(MemoryStore::new()),
            Self::Json { path, journal_floor } => {
                let store = JsonFileStore::new(path)?;
                Arc::new(match journal_floor {
                    Some(bytes) => store.with_journal_floor(*bytes),
                    None => store,
                })
            }
        };
        let backend = StoreBackend::new(persistence, access_policy, confidentiality);
        backend.entity_stores()?;
        backend.component_store()?;
        Ok(backend)
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct PersistenceConfig {
    pub entity_store: PersistenceBackend,
    pub state_store: PersistenceBackend,
}

/// A backend of a service (issue 324): its `type`, by which the runtime
/// finds the factory registered for it - the core's or a plugin's -, and
/// its parameters, written beside the type and read by that factory. A
/// type no factory is registered for, or parameters it cannot read, are
/// refused when the runtime is built.
#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct BackendConfig {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub params: Map<String, Value>,
}

impl BackendConfig {
    /// The backend of the type `kind` with `params`, an object or nothing.
    pub fn new(kind: &str, params: Value) -> Self {
        let params = match params {
            Value::Object(params) => params,
            _ => Map::new(),
        };
        Self { kind: kind.to_string(), params }
    }
}

/// The chain of secret providers, each a backend (`dyn SecretProvider`).
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct SecretsConfig {
    pub providers: Vec<BackendConfig>,
}

/// Without a `secrets` section, secrets come from `local/secrets.json` and
/// then from the environment.
impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            providers: vec![
                BackendConfig::new("file", serde_json::json!({ "path": "local/secrets.json" })),
                BackendConfig::new("env", Value::Null),
            ],
        }
    }
}

/// Which policy decides what may be done to what (issue 11).
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecurityConfig {
    /// Owners do everything with their entities, whom an access list grants
    /// it read and use them, what belongs to the system everybody uses
    /// (`OwnershipPolicy`). The default: nothing is open unless a
    /// deployment says so.
    #[default]
    Ownership,
    /// Everything is allowed to everybody (`AllowAllPolicy`) - for
    /// tests and single-user development setups only.
    AllowAll,
}

/// The chain of user providers, each a backend (`dyn UserProvider`).
/// Without a `users` section the chain is empty and nobody can
/// authenticate: there is no default provider, because the only one that
/// needs no setup accepts everybody.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct UsersConfig {
    pub providers: Vec<BackendConfig>,
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
    pub address: SocketAddr,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { address: SocketAddr::from(([127, 0, 0, 1], 3000)) }
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
/// takes its default. A key the configuration does not know is a section
/// of a plugin's (`plugins`, issue 323); one that no plugin claims is an
/// error when the runtime is built, rather than ignored, so that a misspelt
/// section doesn't silently fall back to its default. Within a section, a
/// key it does not know is an error when it is read.
#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
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
    /// The variants of the texts given to models (issue 57), each by its
    /// name and the directory of its files, one per namespace - `default`
    /// overriding the defaults for every model. Read and checked when the
    /// runtime is built (`PromptCatalogue::add_variant`). None unless
    /// configured: the texts compiled in.
    pub prompts: BTreeMap<String, PathBuf>,
    /// The sections of the plugins, by name, as they were written: each
    /// read by the plugin that claims it (issue 323).
    #[serde(flatten)]
    pub plugins: BTreeMap<String, Value>,
}

/// The storages workspaces are made in (issue 253), each by the name a
/// workspace refers to it with and a backend (`dyn TerminalProvider`), the
/// provider that makes and reaches the place of each (issue 251, part A),
/// and how large a file a person may put into one.
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspacesConfig {
    pub max_file_bytes: u64,
    pub storage: BTreeMap<String, BackendConfig>,
}

/// The largest file a person may put into a workspace unless configured
/// otherwise: 50 MiB.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

impl Default for WorkspacesConfig {
    fn default() -> Self {
        Self { max_file_bytes: DEFAULT_MAX_FILE_BYTES, storage: BTreeMap::new() }
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
            prompts: BTreeMap::new(),
            plugins: BTreeMap::new(),
        }
    }
}

/// Parses a document the way the configuration is parsed, for files a
/// configuration names (e.g. `seed`). Only `true` and `false` are booleans:
/// `yes`, `no`, `on`, `off` and `y` are text, also where nothing says which
/// it should be - the parameters of a backend or the section of a plugin,
/// kept as they were written until they are read.
///
/// # Errors
/// `ConfigError::Parse` if `yaml` is not a `T`; the error names the line and
/// column, but quotes nothing of the input.
pub fn parse_yaml<T: DeserializeOwned>(yaml: &str) -> ConfigResult<T> {
    // By default the parser quotes the offending lines of the input in its
    // error, and a configuration may hold credentials. The location alone
    // is enough to find the mistake.
    let mut options = Options::default();
    options.with_snippet = false;
    options.strict_booleans = true;
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
    /// configured compartments. What a backend is given is checked by the
    /// backend, when it is made (issue 324).
    fn check(&self) -> ConfigResult<()> {
        if let Some(roles) = &self.roles {
            roles.check(&self.compartments).map_err(|e| ConfigError::Invalid(e.to_string()))?;
        }
        for rule in &self.approvals.rules {
            rule.check_restricting().map_err(|e| ConfigError::Invalid(format!("an approval rule for `{}`: {e}", rule.matcher.tool)))?;
        }
        Ok(())
    }

    /// The one `AccessPolicy` of the process, as configured, learning
    /// who belongs to a group from `memberships` (issues 23, 286).
    pub fn create_access_policy(&self, memberships: Arc<dyn Memberships>) -> Arc<dyn AccessPolicy> {
        match self.security {
            SecurityConfig::Ownership => Arc::new(OwnershipPolicy::with_memberships(memberships)),
            SecurityConfig::AllowAll => {
                tracing::warn!("Security policy 'allow_all' is configured: every user may do everything. Development only.");
                Arc::new(AllowAllPolicy::new())
            }
        }
    }

    /// The one `ConfidentialityPolicy` of the process (issue 304): the rules
    /// of the configured compartments and roles - also under `allow_all`,
    /// which allows every action but no flow the rules refuse.
    pub fn create_confidentiality_policy(&self) -> Arc<dyn ConfidentialityPolicy> {
        let rules = ConfidentialityRules::new(self.compartments.clone());
        Arc::new(match &self.roles {
            Some(roles) => rules.with_roles(roles.clone()),
            None => rules,
        })
    }

    /// The store of the entities and of the preconfigured components, as
    /// configured under `persistence.entity_store` (issue 323).
    ///
    /// # Errors
    /// `ConfigError::Store` if the store cannot be opened.
    pub fn open_entity_store(&self, access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>) -> ConfigResult<StoreBackend> {
        self.persistence.entity_store.open(access_policy, confidentiality)
    }

    /// The store of the states and their histories (issue 379), as
    /// configured under `persistence.state_store`.
    ///
    /// # Errors
    /// `ConfigError::Store` if the store cannot be opened.
    pub fn open_state_store(&self, access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>) -> ConfigResult<StoreBackend> {
        self.persistence.state_store.open(access_policy, confidentiality)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role::RoleId;

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
    fn a_backend_is_named_by_its_type_and_keeps_its_parameters_for_its_factory() {
        let config = AppConfig::from_yaml("workspaces:\n  max_file_bytes: 1024\n  storage:\n    dev:\n      type: local\n      root: x\n      shell: true\n      destination: { level: internal }\n").expect("a local storage");
        assert_eq!(config.workspaces.max_file_bytes, 1024);
        let expected = BackendConfig::new("local", serde_json::json!({ "root": "x", "shell": true, "destination": { "level": "internal" } }));
        assert_eq!(config.workspaces.storage.get("dev"), Some(&expected));
        // What the parameters are is the factory's to say, when the runtime
        // is built: a type nobody registers is no error here.
        let unknown = AppConfig::from_yaml("workspaces:\n  storage:\n    main:\n      type: nfs\n      rot: z\n").expect("read as it is written");
        assert_eq!(unknown.workspaces.storage.get("main"), Some(&BackendConfig::new("nfs", serde_json::json!({ "rot": "z" }))));
        assert!(AppConfig::from_yaml("workspaces:\n  storage:\n    main:\n      root: x\n").is_err(), "a type is needed");
        let none = AppConfig::from_yaml("{}\n").expect("nothing configured");
        assert!(none.workspaces.storage.is_empty(), "no storage unless configured");
        assert_eq!(none.workspaces.max_file_bytes, DEFAULT_MAX_FILE_BYTES);
    }

    #[test]
    fn an_unknown_key_is_an_error_naming_it() {
        for yaml in [
            "persistence:\n  entity_stor:\n    type: memory\n",
            "persistence:\n  entity_store:\n    type: json\n    pth: local\n",
            "auth:\n  token_lifetime: 60\n",
        ] {
            let message = AppConfig::from_yaml(yaml).expect_err(yaml).to_string();
            assert!(message.contains("unknown field"), "{yaml:?} gave: {message}");
        }
        let message = AppConfig::from_yaml("persistence:\n  state_store:\n    type: file\n").expect_err("unknown backend").to_string();
        assert!(message.contains("unknown variant"), "gave: {message}");
        // A section it does not know is a plugin's, refused by the runtime
        // if no plugin claims it.
        let config = AppConfig::from_yaml("secret:\n  providers: []\n").unwrap();
        assert_eq!(config.plugins.get("secret"), Some(&serde_json::json!({"providers": []})));
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
            BackendConfig::new("file", serde_json::json!({ "path": "local/secrets.json" })),
            BackendConfig::new("env", Value::Null),
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
        assert_eq!(config.users.providers, [BackendConfig::new("accept_any", Value::Null)]);
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

    #[test]
    fn only_true_and_false_are_booleans() {
        let config = AppConfig::from_yaml("workspaces:\n  storage:\n    main:\n      type: local\n      root: no\n      image: y\n      shell: true\n").unwrap();
        let params = &config.workspaces.storage["main"].params;
        assert_eq!((params["root"].as_str(), params["image"].as_str(), params["shell"].as_bool()), (Some("no"), Some("y"), Some(true)), "kept as text, read as written");
        assert!(AppConfig::from_yaml("auth:\n  secure_cookie: off\n").is_err(), "no boolean where one is needed");
        assert!(!AppConfig::from_yaml("auth:\n  secure_cookie: false\n").unwrap().auth.secure_cookie);
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
        let expected = default_commands().component_nodes.len();
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
    fn roles_are_checked_against_the_compartments() {
        let compartments = "compartments:\n  - name: hr\n";
        let hr = RoleId::new();
        let roles = format!("roles:\n  - id: {hr}\n    name: hr\n    clearances:\n      - {{ level: confidential, compartment: hr }}\n");
        let config = AppConfig::from_yaml(&format!("{compartments}{roles}")).unwrap();
        assert_eq!(config.roles.as_ref().map(|r| r.id_of("hr")), Some(Ok(hr)), "named, kept by its id");
        assert!(AppConfig::default().roles.is_none(), "none unless configured: clearances are not checked");

        let typo = format!("roles:\n  - id: {hr}\n    name: hr\n    clearances:\n      - {{ level: confidential, compartment: hrr }}\n");
        assert!(matches!(AppConfig::from_yaml(&format!("{compartments}{typo}")), Err(ConfigError::Invalid(_))), "a compartment not configured");
        assert!(AppConfig::from_yaml(&format!("{compartments}roles:\n  - name: hr\n")).is_err(), "a role without an id");
    }

    #[test]
    fn the_security_policy_defaults_to_ownership() {
        assert_eq!(AppConfig::default().security, SecurityConfig::Ownership);
        assert_eq!(AppConfig::from_yaml("persistence: {}").unwrap().security, SecurityConfig::Ownership);
        assert_eq!(AppConfig::from_yaml("security:\n  type: allow_all\n").unwrap().security, SecurityConfig::AllowAll);
    }
}
