//! Backends (issue 324): what serves a service - a storage of workspaces
//! (`dyn TerminalProvider`) or of content (`dyn ContentStorage`, issue
//! 282), a user provider (`dyn UserProvider`), a secret provider (`dyn
//! SecretProvider`), what the stores keep their data in (`dyn
//! PersistenceBackend`, issue 446) - named in the configuration by its type
//! (`BackendConfig`) and made by the factory registered under that type for
//! the service: the core's (`Backends::built_in`) or a plugin's
//! (`Plugin::backends`).
//!
//! Built to look like the `ComponentRegistry` where the two do the same: a
//! type is a name, its parameters are read by `config_of_params`, and what
//! a backend needs of the runtime it asks the services it is given for.
//! Unlike a component, a backend is made once for the process, from the
//! configuration, before any container exists - which is why it is none.

use crate::component::{config_of_params, service_of, ComponentError, ComponentResult};
use crate::plugin::Plugin;
use crate::runtime::Services;
use crate::{content, terminal};
use caelumora_model::confidentiality::ConfidentialityPolicy;
use caelumora_model::config::BackendConfig;
use caelumora_model::secret::env::EnvSecretProvider;
use caelumora_model::secret::file::{FileSecretConfig, FileSecretProvider};
use caelumora_model::secret::SecretProvider;
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::store::{EntityStore, PersistenceBackend};
use caelumora_model::type_map::TypeKey;
use caelumora_model::user::User;
use caelumora_model::user_provider::accept_any::AcceptAnyConfig;
use caelumora_model::user_provider::file::FileUserConfig;
use caelumora_model::user_provider::UserProvider;
use serde::Deserialize;
use serde_json::Value;
use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

/// The backends a section of the configuration names, by their names - the
/// storages of workspaces, the storages of content -, which what lies there
/// refers to its backend by. Shown by their names alone: a backend's
/// parameters may name places of the host.
pub struct NamedBackends<T: ?Sized>(BTreeMap<String, Arc<T>>);

/// A backend that something kept names as where it lies - a workspace, an artifact's version, an
/// attachment -, since removed from the configuration (issue 466): the server's failure, not the
/// request's, however the kept thing was reached. Its message names the backend, which is the
/// configuration's: for the log, not for users or models.
#[derive(Debug, thiserror::Error)]
#[error("the storage '{0}' is no longer configured")]
pub struct StorageGone(pub String);

/// Why no backend of a `NamedBackends` could be chosen (`chosen`), as the
/// end of a sentence about the parameter that names it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unchosen {
    #[error("names '{0}', which is not configured")]
    Unknown(String),
    #[error("has to name one where there are several")]
    Unnamed,
}

impl<T: ?Sized> NamedBackends<T> {
    /// The backend `name`, if one is configured.
    pub fn get(&self, name: &str) -> Option<&Arc<T>> {
        self.0.get(name)
    }

    /// The backend `name` that something kept names as where it lies.
    ///
    /// # Errors
    /// `StorageGone` if it is no longer configured.
    pub fn kept_in(&self, name: &str) -> Result<&Arc<T>, StorageGone> {
        self.0.get(name).ok_or_else(|| StorageGone(name.to_string()))
    }

    /// The one to use, with its name: `named`, which has to be one of them, or the one there is -
    /// `None` where none is configured.
    ///
    /// # Errors
    /// `Unknown` if `named` is none of them, `Unnamed` if nothing is named among several.
    pub fn chosen(&self, named: Option<&str>) -> Result<Option<(&str, &Arc<T>)>, Unchosen> {
        let chosen = match named {
            Some(name) => Some(
                self.0.get_key_value(name).ok_or_else(|| Unchosen::Unknown(name.to_string()))?,
            ),
            None if self.0.len() > 1 => return Err(Unchosen::Unnamed),
            None => self.0.iter().next(),
        };
        Ok(chosen.map(|(name, backend)| (name.as_str(), backend)))
    }

    /// Every backend, by its name.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Arc<T>)> {
        self.0.iter().map(|(name, backend)| (name.as_str(), backend))
    }
}

/// None configured.
impl<T: ?Sized> Default for NamedBackends<T> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<T: ?Sized> fmt::Debug for NamedBackends<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

/// What makes a backend of `T`: given the name it is configured under - its
/// position in a chain -, its parameters and the services made before it.
type BackendFactory<T> =
    Box<dyn Fn(&str, &Value, &Services) -> ComponentResult<Arc<T>> + Send + Sync>;

/// The factories of the backends of one service, by type.
type Factories<T> = BTreeMap<String, BackendFactory<T>>;

/// The backends there are, by the service they serve and their type. Filled
/// before anything is made (`with_plugins`), and asked once, when the
/// runtime is built, for the backends the configuration names.
#[derive(Default)]
pub struct Backends {
    /// A `Factories<T>` for each service `T`, with its name for messages.
    factories: HashMap<TypeId, (&'static str, Box<dyn Any + Send + Sync>)>,
}

impl fmt::Debug for Backends {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut services: Vec<_> = self.factories.values().map(|(name, _)| *name).collect();
        services.sort_unstable();
        f.debug_struct("Backends").field("services", &services).finish()
    }
}

impl Backends {
    pub fn new() -> Self {
        Self::default()
    }

    /// The core's backends: the stores in memory, the storages of
    /// `terminal` and of `content`, the secrets of a file and of the environment, and the
    /// users of a file and `accept_any`. The stores in JSON files are a
    /// plugin's (`caelumora-store-json`, issue 446).
    pub fn built_in() -> Self {
        let mut backends = Self::new();
        backends.register::<dyn PersistenceBackend, _>("memory", |_, params, _| {
            config_of_params::<NoParams>(params)?;
            Ok(Arc::new(MemoryStore::new()))
        });
        terminal::register_backends(&mut backends);
        content::register_backends(&mut backends);
        backends.register::<dyn SecretProvider, _>("file", |_, params, _| {
            let config: FileSecretConfig = config_of_params(params)?;
            Ok(Arc::new(FileSecretProvider::new(config.path)))
        });
        backends.register::<dyn SecretProvider, _>("env", |_, params, _| {
            config_of_params::<NoParams>(params)?;
            Ok(Arc::new(EnvSecretProvider::new()))
        });
        backends.register::<dyn UserProvider, _>("file", |_, params, services| {
            let config: FileUserConfig = config_of_params(params)?;
            let roles = service_of::<dyn ConfidentialityPolicy>(services)?.roles();
            let provider =
                config.provider(roles).map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
            Ok(Arc::new(provider))
        });
        backends.register::<dyn UserProvider, _>("accept_any", |_, params, services| {
            let config: AcceptAnyConfig = config_of_params(params)?;
            let users = service_of::<dyn EntityStore<User>>(services)?;
            let roles = service_of::<dyn ConfidentialityPolicy>(services)?.roles();
            let provider = config.provider(Arc::clone(users), roles).map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
            tracing::warn!("User provider 'accept_any' is enabled: ANY username and password will be accepted. \
                Development only - never expose this server to a network.");
            Ok(Arc::new(provider))
        });
        backends
    }

    /// The core's backends and those `plugins` bring.
    pub fn with_plugins(plugins: &[&dyn Plugin]) -> Self {
        let mut backends = Self::built_in();
        for plugin in plugins {
            plugin.backends(&mut backends);
        }
        backends
    }

    /// Registers `factory` as what makes the backends of the type `kind`
    /// for the service `T`, in place of one registered before.
    pub fn register<T, F>(&mut self, kind: &str, factory: F)
    where
        T: ?Sized + 'static,
        F: Fn(&str, &Value, &Services) -> ComponentResult<Arc<T>> + Send + Sync + 'static,
    {
        let (_, factories) = self
            .factories
            .entry(TypeId::of::<T>())
            .or_insert_with(|| (TypeKey::of::<T>().name(), Box::new(Factories::<T>::new())));
        if let Some(factories) = factories.downcast_mut::<Factories<T>>() {
            factories.insert(kind.to_string(), Box::new(factory));
        }
    }

    /// The backends of `T` that `configs` names, by their names; `what`
    /// names one of them in messages - "storage of workspaces".
    ///
    /// # Errors
    /// `ComponentError::Backend` naming the first that cannot be made.
    pub fn make_named<T: ?Sized + 'static>(
        &self,
        what: &str,
        configs: &BTreeMap<String, BackendConfig>,
        services: &Services,
    ) -> ComponentResult<NamedBackends<T>> {
        configs
            .iter()
            .map(|(name, config)| {
                Ok((
                    name.clone(),
                    self.make(&format!("the {what} '{name}'"), name, config, services)?,
                ))
            })
            .collect::<ComponentResult<_>>()
            .map(NamedBackends)
    }

    /// The chain of backends of `T` that `configs` names, in its order;
    /// `what` names one of them in messages - "secret provider".
    ///
    /// # Errors
    /// `ComponentError::Backend` naming the first that cannot be made.
    pub fn make_chain<T: ?Sized + 'static>(
        &self,
        what: &str,
        configs: &[BackendConfig],
        services: &Services,
    ) -> ComponentResult<Vec<Arc<T>>> {
        configs
            .iter()
            .enumerate()
            .map(|(at, config)| {
                let position = (at + 1).to_string();
                self.make(&format!("the {what} {position}"), &position, config, services)
            })
            .collect()
    }

    /// The one backend of `T` that `config` names, configured as `name`;
    /// `what` names it in messages - "entity store".
    ///
    /// # Errors
    /// `ComponentError::Backend` naming it if it cannot be made.
    pub fn make_one<T: ?Sized + 'static>(
        &self,
        what: &str,
        name: &str,
        config: &BackendConfig,
        services: &Services,
    ) -> ComponentResult<Arc<T>> {
        self.make(&format!("the {what}"), name, config, services)
    }

    /// The backend `config` names, configured as `name`, described as
    /// `backend` in messages.
    fn make<T: ?Sized + 'static>(
        &self,
        backend: &str,
        name: &str,
        config: &BackendConfig,
        services: &Services,
    ) -> ComponentResult<Arc<T>> {
        let factories = self
            .factories
            .get(&TypeId::of::<T>())
            .and_then(|(_, factories)| factories.downcast_ref::<Factories<T>>());
        let made = match factories.and_then(|factories| factories.get(&config.kind)) {
            Some(factory) => factory(name, &Value::Object(config.params.clone()), services),
            None => {
                let known: Vec<_> = factories
                    .into_iter()
                    .flat_map(|factories| factories.keys().map(String::as_str))
                    .collect();
                Err(ComponentError::InvalidConfig(format!(
                    "no backend of this type is known; known are: {}",
                    known.join(", ")
                )))
            }
        };
        made.map_err(|cause| ComponentError::Backend {
            backend: backend.to_string(),
            kind: config.kind.clone(),
            cause: Box::new(cause),
        })
    }
}

/// The parameters of a backend that takes none: an object without fields,
/// so that a misspelt one is refused rather than ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::role::{RoleId, Roles};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{AllowAllPolicy, SecurityContext};
    use caelumora_model::store::{EntityStores, Persistence, Policies};
    use caelumora_model::user::UserId;
    use caelumora_model::user_provider::{Credential, UserManager};
    use serde_json::json;
    use std::env;
    use std::fs;

    /// A service of a plugin's, with a backend of its own.
    trait Greeter: Send + Sync {
        fn greet(&self) -> String;
    }

    struct Fixed(String);

    impl Greeter for Fixed {
        fn greet(&self) -> String {
            self.0.clone()
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixedConfig {
        greeting: String,
    }

    fn greeters() -> Backends {
        let mut backends = Backends::new();
        backends.register::<dyn Greeter, _>("fixed", |name, params, _| {
            let config: FixedConfig = config_of_params(params)?;
            Ok(Arc::new(Fixed(format!("{} from {name}", config.greeting))))
        });
        backends
    }

    #[test]
    fn a_backend_is_made_by_the_factory_of_its_type_from_its_parameters() {
        let configs =
            [("front".to_string(), BackendConfig::new("fixed", json!({ "greeting": "hello" })))]
                .into();
        let made =
            greeters().make_named::<dyn Greeter>("greeter", &configs, &Services::empty()).unwrap();
        assert_eq!(
            made.get("front").unwrap().greet(),
            "hello from front",
            "given its name and its parameters"
        );
        let chain = greeters()
            .make_chain::<dyn Greeter>(
                "greeter",
                &[BackendConfig::new("fixed", json!({ "greeting": "hi" }))],
                &Services::empty(),
            )
            .unwrap();
        assert_eq!(chain[0].greet(), "hi from 1", "in a chain, named by its position");
    }

    #[test]
    fn of_the_backends_by_name_the_one_named_is_chosen_or_the_one_there_is() {
        let named = |names: &[&str]| {
            let configs = names
                .iter()
                .map(|name| {
                    (name.to_string(), BackendConfig::new("fixed", json!({ "greeting": "hi" })))
                })
                .collect();
            greeters().make_named::<dyn Greeter>("greeter", &configs, &Services::empty()).unwrap()
        };
        let front = named(&["front"]);
        let (name, greeter) = front.chosen(None).unwrap().expect("the one there is");
        assert_eq!(
            (name, greeter.greet().as_str()),
            ("front", "hi from front"),
            "with the backend itself"
        );
        assert!(named(&[]).chosen(None).unwrap().is_none(), "none configured");
        let several = named(&["a", "b"]);
        assert_eq!(
            several.chosen(None).err(),
            Some(Unchosen::Unnamed),
            "which one, among several, must be said"
        );
        assert_eq!(several.chosen(Some("b")).unwrap().map(|(name, _)| name), Some("b"));
        assert_eq!(
            several.chosen(Some("c")).err(),
            Some(Unchosen::Unknown("c".to_string())),
            "not one that is not there"
        );
        assert_eq!(several.iter().map(|(name, _)| name).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(format!("{several:?}"), r#"["a", "b"]"#, "shown by their names alone");
    }

    #[test]
    fn an_unknown_type_or_wrong_parameters_are_refused_naming_the_backend() {
        let unknown = [("front".to_string(), BackendConfig::new("loud", json!({})))].into();
        let refused = greeters()
            .make_named::<dyn Greeter>("greeter", &unknown, &Services::empty())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("the greeter 'front' (loud)") && refused.contains("known are: fixed"),
            "{refused}"
        );
        let wrong = [BackendConfig::new("fixed", json!({ "greting": "hello" }))];
        let refused = greeters()
            .make_chain::<dyn Greeter>("greeter", &wrong, &Services::empty())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("the greeter 1 (fixed)") && refused.contains("greting"),
            "{refused}"
        );
        let other_service = Backends::built_in()
            .make_chain::<dyn Greeter>(
                "greeter",
                &[BackendConfig::new("env", json!({}))],
                &Services::empty(),
            )
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            other_service.contains("no backend of this type is known"),
            "the types of another service are not its: {other_service}"
        );
    }

    /// The stores keep their data in memory unless a plugin brings another
    /// backend (issue 446); a store that cannot be made is refused naming
    /// it.
    #[tokio::test]
    async fn the_stores_keep_their_data_in_memory() {
        let memory = BackendConfig::new("memory", json!(null));
        let kept = Backends::built_in()
            .make_one::<dyn PersistenceBackend>(
                "entity store",
                "entity_store",
                &memory,
                &Services::empty(),
            )
            .unwrap();
        let stores = Persistence::over(kept).expect("its states, too");
        stores.entity_store::<User>(&Policies::permitting_all()).expect("any kind is kept");

        let with_params = BackendConfig::new("memory", json!({ "path": "x" }));
        let refused = Backends::built_in()
            .make_one::<dyn PersistenceBackend>(
                "entity store",
                "entity_store",
                &with_params,
                &Services::empty(),
            )
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("the entity store (memory)") && refused.contains("path"),
            "{refused}"
        );
        let unknown = BackendConfig::new("json", json!({ "path": "x" }));
        let refused = Backends::built_in()
            .make_one::<dyn PersistenceBackend>(
                "state store",
                "state_store",
                &unknown,
                &Services::empty(),
            )
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("the state store (json)") && refused.contains("known are: memory"),
            "without its plugin: {refused}"
        );
    }

    #[tokio::test]
    async fn the_secrets_come_from_a_file_and_from_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secrets.json");
        fs::write(&file, r#"{"CAELUMORA_TEST_BACKEND_FILE": "from-file"}"#).unwrap();
        env::set_var("CAELUMORA_TEST_BACKEND_ENV", "from-env");
        let configs = [
            BackendConfig::new("file", json!({ "path": file })),
            BackendConfig::new("env", json!(null)),
        ];
        let providers = Backends::built_in()
            .make_chain::<dyn SecretProvider>("secret provider", &configs, &Services::empty())
            .unwrap();
        let secrets = SecretManager::new(providers, Arc::new(AllowAllPolicy::new()));
        let context = SecurityContext::default();
        assert_eq!(
            secrets.resolve(&context, "CAELUMORA_TEST_BACKEND_FILE").await.unwrap(),
            "from-file"
        );
        assert_eq!(
            secrets.resolve(&context, "CAELUMORA_TEST_BACKEND_ENV").await.unwrap(),
            "from-env"
        );
        env::remove_var("CAELUMORA_TEST_BACKEND_ENV");
        let misspelt = Backends::built_in().make_chain::<dyn SecretProvider>(
            "secret provider",
            &[BackendConfig::new("env", json!({ "prefix": "X" }))],
            &Services::empty(),
        );
        assert!(misspelt.is_err(), "env takes no parameters");
    }

    #[tokio::test]
    async fn the_users_of_a_file_log_in_with_the_roles_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("users.yaml");
        let alice = UserId::new();
        // The hash of "wonderland", made cheaply.
        let hash = "$argon2id$v=19$m=8,t=1,p=1$Y2FlbHVtb3JhLXRlc3RzIQ$O5O4BCrndw3DwlDMCkis9v7MtqH5yH38HhXnJ6aRr2M";
        fs::write(&file, format!("users:\n  - id: {alice}\n    username: alice\n    roles: [hr]\n    password_hash: \"{hash}\"\n")).unwrap();
        let users = EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
            .unwrap()
            .users;
        let hr = RoleId::new();
        let roles = Roles::new([(hr, "hr".to_string(), vec![])]).unwrap();
        let confidentiality: Arc<dyn ConfidentialityPolicy> =
            Arc::new(ConfidentialityRules::default().with_roles(roles));
        let services = Services::empty().with(Arc::clone(&users)).with(confidentiality);
        let configs = [BackendConfig::new("file", json!({ "path": file }))];
        let providers = Backends::built_in()
            .make_chain::<dyn UserProvider>("user provider", &configs, &services)
            .unwrap();
        let manager = UserManager::new(providers, users, Arc::new(()));
        let found = manager
            .authenticate("alice", &Credential::Password("wonderland"))
            .await
            .expect("her password");
        assert_eq!((found.id, found.roles), (alice, vec![hr]));

        let missing =
            [BackendConfig::new("file", json!({ "path": dir.path().join("nobody.yaml") }))];
        let refused = Backends::built_in()
            .make_chain::<dyn UserProvider>("user provider", &missing, &services)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("the user provider 1 (file)"),
            "a file that is not there is refused at startup: {refused}"
        );
    }

    #[tokio::test]
    async fn accept_any_gives_the_configured_roles_and_refuses_others() {
        let users = EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
            .unwrap()
            .users;
        let hr = RoleId::new();
        let roles = Roles::new([(hr, "hr".to_string(), vec![])]).unwrap();
        let confidentiality: Arc<dyn ConfidentialityPolicy> =
            Arc::new(ConfidentialityRules::default().with_roles(roles));
        let services = Services::empty().with(Arc::clone(&users)).with(confidentiality);
        let configs = [BackendConfig::new("accept_any", json!({ "default_roles": ["hr"] }))];
        let providers = Backends::built_in()
            .make_chain::<dyn UserProvider>("user provider", &configs, &services)
            .unwrap();
        let manager = UserManager::new(providers, users, Arc::new(()));
        let alice =
            manager.authenticate("alice", &Credential::Password("x")).await.expect("accepted");
        assert_eq!(alice.roles, [hr]);

        let unknown = [BackendConfig::new("accept_any", json!({ "default_roles": ["staff"] }))];
        let refused = Backends::built_in()
            .make_chain::<dyn UserProvider>("user provider", &unknown, &services)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("the user provider 1 (accept_any)") && refused.contains("staff"),
            "{refused}"
        );
    }
}
