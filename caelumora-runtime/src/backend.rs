//! Backends (issue 324): what serves a service - a storage of workspaces
//! (`dyn TerminalProvider`) or of a plugin's (a storage of artifacts, issue
//! 328), a user provider (`dyn UserProvider`), a secret provider (`dyn
//! SecretProvider`) -
//! named in the configuration by its type (`BackendConfig`) and made by the
//! factory registered under that type for the service: the core's
//! (`Backends::built_in`) or a plugin's (`Plugin::backends`).
//!
//! Built to look like the `ComponentRegistry` where the two do the same: a
//! type is a name, its parameters are read by `config_of_params`, and what
//! a backend needs of the runtime it asks the services it is given for.
//! Unlike a component, a backend is made once for the process, from the
//! configuration, before any container exists - which is why it is none.
//! The persistence (`PersistenceBackend`) is no backend of this kind: its
//! stores are generic over the kinds they keep, and stay the core's.

use crate::component::{config_of_params, service_of, ComponentError, ComponentResult};
use crate::plugin::Plugin;
use crate::runtime::Services;
use caelumora_model::config::BackendConfig;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use caelumora_model::secret::env::EnvSecretProvider;
use caelumora_model::secret::file::{FileSecretConfig, FileSecretProvider};
use caelumora_model::secret::SecretProvider;
use caelumora_model::store::EntityStore;
use caelumora_model::type_map::TypeKey;
use caelumora_model::user::User;
use caelumora_model::user_provider::accept_any::AcceptAnyConfig;
use caelumora_model::user_provider::UserProvider;
use serde::Deserialize;
use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

/// What makes a backend of `T`: given the name it is configured under - its
/// position in a chain -, its parameters and the services made before it.
type BackendFactory<T> = Box<dyn Fn(&str, &serde_json::Value, &Services) -> ComponentResult<Arc<T>> + Send + Sync>;

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

    /// The core's backends: the storages of `terminal`, the secrets of a
    /// file and of the environment, and `accept_any`.
    pub fn built_in() -> Self {
        let mut backends = Self::new();
        crate::terminal::register_backends(&mut backends);
        backends.register::<dyn SecretProvider, _>("file", |_, params, _| {
            let config: FileSecretConfig = config_of_params(params)?;
            Ok(Arc::new(FileSecretProvider::new(config.path)))
        });
        backends.register::<dyn SecretProvider, _>("env", |_, params, _| {
            config_of_params::<NoParams>(params)?;
            Ok(Arc::new(EnvSecretProvider::new()))
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
        F: Fn(&str, &serde_json::Value, &Services) -> ComponentResult<Arc<T>> + Send + Sync + 'static,
    {
        let (_, factories) = self.factories.entry(TypeId::of::<T>())
            .or_insert_with(|| (TypeKey::of::<T>().name, Box::new(Factories::<T>::new())));
        if let Some(factories) = factories.downcast_mut::<Factories<T>>() {
            factories.insert(kind.to_string(), Box::new(factory));
        }
    }

    /// The backends of `T` that `configs` names, by their names; `what`
    /// names one of them in messages - "storage of workspaces".
    ///
    /// # Errors
    /// `ComponentError::Backend` naming the first that cannot be made.
    pub fn make_named<T: ?Sized + 'static>(&self, what: &str, configs: &BTreeMap<String, BackendConfig>, services: &Services) -> ComponentResult<BTreeMap<String, Arc<T>>> {
        configs.iter()
            .map(|(name, config)| Ok((name.clone(), self.make(&format!("the {what} '{name}'"), name, config, services)?)))
            .collect()
    }

    /// The chain of backends of `T` that `configs` names, in its order;
    /// `what` names one of them in messages - "secret provider".
    ///
    /// # Errors
    /// `ComponentError::Backend` naming the first that cannot be made.
    pub fn make_chain<T: ?Sized + 'static>(&self, what: &str, configs: &[BackendConfig], services: &Services) -> ComponentResult<Vec<Arc<T>>> {
        configs.iter().enumerate()
            .map(|(at, config)| {
                let position = (at + 1).to_string();
                self.make(&format!("the {what} {position}"), &position, config, services)
            })
            .collect()
    }

    /// The backend `config` names, configured as `name`, described as
    /// `backend` in messages.
    fn make<T: ?Sized + 'static>(&self, backend: &str, name: &str, config: &BackendConfig, services: &Services) -> ComponentResult<Arc<T>> {
        let factories = self.factories.get(&TypeId::of::<T>()).and_then(|(_, factories)| factories.downcast_ref::<Factories<T>>());
        let made = match factories.and_then(|factories| factories.get(&config.kind)) {
            Some(factory) => factory(name, &serde_json::Value::Object(config.params.clone()), services),
            None => {
                let known: Vec<_> = factories.into_iter().flat_map(|factories| factories.keys().map(String::as_str)).collect();
                Err(ComponentError::InvalidConfig(format!("no backend of this type is known; known are: {}", known.join(", "))))
            }
        };
        made.map_err(|cause| ComponentError::Backend { backend: backend.to_string(), kind: config.kind.clone(), cause: Box::new(cause) })
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
    use caelumora_model::security::{DummySecurityManager, SecurityContext};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user_provider::{Credential, UserManager};
    use serde_json::json;

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
        let configs = [("front".to_string(), BackendConfig::new("fixed", json!({ "greeting": "hello" })))].into();
        let made = greeters().make_named::<dyn Greeter>("greeter", &configs, &Services::empty()).unwrap();
        assert_eq!(made["front"].greet(), "hello from front", "given its name and its parameters");
        let chain = greeters().make_chain::<dyn Greeter>("greeter", &[BackendConfig::new("fixed", json!({ "greeting": "hi" }))], &Services::empty()).unwrap();
        assert_eq!(chain[0].greet(), "hi from 1", "in a chain, named by its position");
    }

    #[test]
    fn an_unknown_type_or_wrong_parameters_are_refused_naming_the_backend() {
        let unknown = [("front".to_string(), BackendConfig::new("loud", json!({})))].into();
        let refused = greeters().make_named::<dyn Greeter>("greeter", &unknown, &Services::empty()).map(|_| ()).unwrap_err().to_string();
        assert!(refused.contains("the greeter 'front' (loud)") && refused.contains("known are: fixed"), "{refused}");
        let wrong = [BackendConfig::new("fixed", json!({ "greting": "hello" }))];
        let refused = greeters().make_chain::<dyn Greeter>("greeter", &wrong, &Services::empty()).map(|_| ()).unwrap_err().to_string();
        assert!(refused.contains("the greeter 1 (fixed)") && refused.contains("greting"), "{refused}");
        let other_service = Backends::built_in().make_chain::<dyn Greeter>("greeter", &[BackendConfig::new("env", json!({}))], &Services::empty()).map(|_| ()).unwrap_err().to_string();
        assert!(other_service.contains("no backend of this type is known"), "the types of another service are not its: {other_service}");
    }

    #[tokio::test]
    async fn the_secrets_come_from_a_file_and_from_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secrets.json");
        std::fs::write(&file, r#"{"CAELUMORA_TEST_BACKEND_FILE": "from-file"}"#).unwrap();
        std::env::set_var("CAELUMORA_TEST_BACKEND_ENV", "from-env");
        let configs = [BackendConfig::new("file", json!({ "path": file })), BackendConfig::new("env", json!(null))];
        let providers = Backends::built_in().make_chain::<dyn SecretProvider>("secret provider", &configs, &Services::empty()).unwrap();
        let secrets = SecretManager::new(providers, Arc::new(DummySecurityManager::new()));
        let context = SecurityContext::default();
        assert_eq!(secrets.resolve(&context, "CAELUMORA_TEST_BACKEND_FILE").await.unwrap(), "from-file");
        assert_eq!(secrets.resolve(&context, "CAELUMORA_TEST_BACKEND_ENV").await.unwrap(), "from-env");
        std::env::remove_var("CAELUMORA_TEST_BACKEND_ENV");
        let misspelt = Backends::built_in().make_chain::<dyn SecretProvider>("secret provider", &[BackendConfig::new("env", json!({ "prefix": "X" }))], &Services::empty());
        assert!(misspelt.is_err(), "env takes no parameters");
    }

    #[tokio::test]
    async fn accept_any_gives_the_configured_roles_and_refuses_others() {
        let users = Arc::new(MemoryStore::new()).entity_stores().users;
        let hr = RoleId::new();
        let roles = Roles::new([(hr, "hr".to_string(), vec![])]).unwrap();
        let confidentiality: Arc<dyn ConfidentialityPolicy> = Arc::new(ConfidentialityRules::default().with_roles(roles));
        let services = Services::empty().with(Arc::clone(&users)).with(confidentiality);
        let configs = [BackendConfig::new("accept_any", json!({ "default_roles": ["hr"] }))];
        let providers = Backends::built_in().make_chain::<dyn UserProvider>("user provider", &configs, &services).unwrap();
        let manager = UserManager::new(providers, users, Arc::new(()));
        let alice = manager.authenticate("alice", &Credential::Password("x")).await.expect("accepted");
        assert_eq!(alice.roles, [hr]);

        let unknown = [BackendConfig::new("accept_any", json!({ "default_roles": ["staff"] }))];
        let refused = Backends::built_in().make_chain::<dyn UserProvider>("user provider", &unknown, &services).map(|_| ()).unwrap_err().to_string();
        assert!(refused.contains("the user provider 1 (accept_any)") && refused.contains("staff"), "{refused}");
    }
}
