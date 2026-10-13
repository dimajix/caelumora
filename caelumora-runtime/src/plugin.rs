//! Plugins (issue 323): subjects joined to the core as crates linked into
//! the program, not loaded at run time. A plugin registers what it brings
//! when the runtime is built, in two steps: first the backends of the
//! core's services (`Backends`, issue 324), before anything is made; then,
//! with the core's services and managers made (`RuntimeBuilder`), its
//! component kinds, the stores of its kinds and the managers that keep
//! them, as services, the members of a scope, its sections of the
//! configuration and of the seed. Which plugins are in is the program's
//! choice; the core knows none.

use crate::backend::Backends;
use crate::runtime::RuntimeBuilder;

/// A subject joined to the core, after the pattern of Bevy's
/// `Plugin::build`.
pub trait Plugin: Send + Sync {
    /// Its name, as messages name it.
    fn name(&self) -> &'static str;

    /// Registers the backends it brings with `backends` - before the
    /// services they serve are made, so before `register`. None unless it
    /// says otherwise.
    fn backends(&self, _backends: &mut Backends) {}

    /// Registers what it brings with `runtime`.
    ///
    /// # Errors
    /// If its section of the configuration cannot be read, or a store of
    /// its cannot be opened - the process does not start then.
    fn register(&self, runtime: &mut RuntimeBuilder) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{ComponentContainer, ComponentContext, ComponentFacets, ComponentKind};
    use crate::testing::test_registry;
    use crate::runtime::RuntimeManager;
    use crate::seed::SeedSection;
    use crate::session::SessionManager;
    use crate::view::Viewer;
    use async_trait::async_trait;
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::config::AppConfig;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::define_id;
    use caelumora_model::group::Group;
    use caelumora_model::removal::{Deletable, Removal};
    use caelumora_model::secret::{SecretManager, SecretProvider, SecretResult};
    use crate::component::config_of_params;
    use caelumora_model::security::{DummySecurityManager, Secured, SecurityAttributes, SecurityContext};
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::query::NoField;
    use caelumora_model::store::{EntityKind, EntityStore, EntityStoreFactory, NoEntity, StoreError, Stored};
    use caelumora_model::type_map::TypeKey;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;

    define_id!(NoteId);

    /// The kind of the plugin.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Note {
        id: NoteId,
        text: String,
        #[serde(default)]
        security_attributes: SecurityAttributes,
        #[serde(default)]
        deleted: Option<Removal>,
    }

    impl Secured for Note {
        fn security_attributes(&self) -> &SecurityAttributes {
            &self.security_attributes
        }
    }

    impl Stored for Note {
        const KIND: EntityKind = EntityKind::new("note");
        type Id = NoteId;
        type Within = NoEntity;
        type Field = NoField;

        fn id(&self) -> NoteId {
            self.id
        }

        fn deleted(&self) -> Option<&Removal> {
            self.deleted.as_ref()
        }
    }

    impl Deletable for Note {
        fn set_deleted(&mut self, deleted: Option<Removal>) {
            self.deleted = deleted;
        }
    }

    /// The manager of the plugin's kind, offered as a service.
    #[derive(Debug)]
    struct Notebook {
        notes: Arc<dyn EntityStore<Note>>,
        greeting: String,
    }

    #[async_trait]
    impl SeedSection for Notebook {
        async fn seed(&self, section: serde_json::Value) -> anyhow::Result<()> {
            for note in serde_json::from_value::<Vec<Note>>(section)? {
                self.notes.create(&SecurityContext::system(), &note).await?;
            }
            Ok(())
        }
    }

    /// The plugin's section of the configuration.
    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct NotesConfig {
        greeting: String,
    }

    struct Notes;

    impl Plugin for Notes {
        fn name(&self) -> &'static str {
            "notes"
        }

        fn register(&self, runtime: &mut RuntimeBuilder) -> anyhow::Result<()> {
            let config: NotesConfig = runtime.config("notes")?;
            let notebook = Arc::new(Notebook { notes: runtime.entity_store::<Note>()?, greeting: config.greeting });
            runtime.provide(Arc::clone(&notebook));
            runtime.seed_section("notes", notebook)?;
            runtime.components().register(ComponentKind::new("notebook").provides::<Notebook>().uses::<Notebook>(), |_, context| {
                let notebook = Arc::clone(context.service::<Notebook>()?);
                Ok(ComponentFacets::empty().with(notebook))
            });
            Ok(())
        }
    }

    /// A plugin of a kind named as the core's groups are.
    struct Impostor;

    impl Plugin for Impostor {
        fn name(&self) -> &'static str {
            "impostor"
        }

        fn register(&self, runtime: &mut RuntimeBuilder) -> anyhow::Result<()> {
            runtime.entity_store::<Group>()?;
            #[derive(Debug, Clone, Serialize, Deserialize)]
            struct Groupie(Note);
            impl Secured for Groupie {
                fn security_attributes(&self) -> &SecurityAttributes {
                    &self.0.security_attributes
                }
            }
            impl Stored for Groupie {
                const KIND: EntityKind = Group::KIND;
                type Id = NoteId;
                type Within = NoEntity;
                type Field = NoField;
                fn id(&self) -> NoteId {
                    self.0.id
                }
            }
            impl Deletable for Groupie {
                fn set_deleted(&mut self, _deleted: Option<Removal>) {}
            }
            runtime.entity_store::<Groupie>()?;
            Ok(())
        }
    }

    /// A plugin that brings a backend of a service of the core: secrets from
    /// a place of its own.
    struct Vault;

    #[derive(Debug)]
    struct VaultSecrets(String);

    #[async_trait]
    impl SecretProvider for VaultSecrets {
        async fn get_secret(&self, _key: &str) -> SecretResult<Option<String>> {
            Ok(Some(self.0.clone()))
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct VaultConfig {
        secret: String,
    }

    impl Plugin for Vault {
        fn name(&self) -> &'static str {
            "vault"
        }

        fn backends(&self, backends: &mut Backends) {
            backends.register::<dyn SecretProvider, _>("vault", |_, params, _| {
                let config: VaultConfig = config_of_params(params)?;
                Ok(Arc::new(VaultSecrets(config.secret)))
            });
        }

        fn register(&self, _runtime: &mut RuntimeBuilder) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn builder(store: &Arc<MemoryStore>, config: &AppConfig) -> anyhow::Result<RuntimeBuilder> {
        let security = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&security)));
        RuntimeBuilder::new(Arc::<MemoryStore>::clone(store), Arc::<MemoryStore>::clone(store), secrets, security, Arc::new(ConfidentialityRules::default()), config, test_registry(Arc::<MemoryStore>::clone(store)), Backends::built_in())
    }

    fn built(config: &AppConfig, plugin: &dyn Plugin) -> anyhow::Result<RuntimeManager> {
        let mut builder = builder(&Arc::new(MemoryStore::new()), config)?;
        builder.plugin(plugin)?;
        builder.build()
    }

    #[tokio::test]
    async fn a_plugin_brings_its_kind_its_manager_its_component_and_its_sections() {
        let store = Arc::new(MemoryStore::new());
        let config = AppConfig::from_yaml("notes:\n  greeting: hello\n").unwrap();
        let mut builder = builder(&store, &config).unwrap();
        builder.plugin(&Notes).unwrap();
        let runtime = Arc::new(builder.build().unwrap());

        let notebook = runtime.services().get::<Notebook>().expect("its manager, a service");
        assert_eq!(notebook.greeting, "hello", "its section of the configuration");
        let views = Arc::new(SessionManager::new(Arc::clone(&runtime))).views(Viewer::new(SecurityContext::system()));
        assert!(views.service::<Notebook>().is_some(), "reached by its views");

        // Its component kind, given the manager it uses.
        let context = ComponentContext::new(SecurityContext::system(), runtime.services().clone());
        let config = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("notebook", "notebook")] };
        ComponentContainer::from_config(&config, &[TypeKey::of::<Notebook>()], runtime.component_registry(), &context).await.expect("built with its service");

        // Its seed section, into the store of its kind - the one the backend
        // gives whoever asks.
        let dir = tempfile::tempdir().unwrap();
        let seed = dir.path().join("seed.yaml");
        std::fs::write(&seed, "notes:\n- id: 3f9a5e56-e5ea-43f9-9114-2098a6f417a9\n  text: remember\n").unwrap();
        runtime.seed(&[seed]).await.unwrap();
        let notes = store.entity_store::<Note>().unwrap();
        let note = notes.get(&SecurityContext::system(), "3f9a5e56-e5ea-43f9-9114-2098a6f417a9".parse().unwrap()).await.expect("seeded");
        assert_eq!(note.text, "remember");
    }

    #[tokio::test]
    async fn a_plugin_brings_a_backend_of_a_service_of_the_core() {
        let config = AppConfig::from_yaml("secrets:\n  providers:\n    - type: vault\n      secret: s3cr3t\n").unwrap();
        let runtime = RuntimeBuilder::from_config(&config, &[&Vault]).unwrap().build().unwrap();
        assert_eq!(runtime.secret_manager().resolve(&SecurityContext::system(), "ANY").await.unwrap(), "s3cr3t", "made by the plugin's factory");
        let without = RuntimeBuilder::from_config(&config, &[]).map(|_| ()).expect_err("nobody brings it");
        assert!(format!("{without:#}").contains("the secret provider 1 (vault)"), "{without:#}");
    }

    #[test]
    fn a_section_of_the_configuration_nobody_claims_is_an_error() {
        let refused = built(&AppConfig::from_yaml("notez:\n  greeting: hello\n").unwrap(), &Notes).expect_err("a typo");
        assert!(refused.to_string().contains("'notez'"), "{refused}");
        let unreadable = built(&AppConfig::from_yaml("notes:\n  greting: hello\n").unwrap(), &Notes).expect_err("not its section");
        assert!(format!("{unreadable:#}").contains("greting"), "{unreadable:#}");
        let mut twice = builder(&Arc::new(MemoryStore::new()), &AppConfig::default()).unwrap();
        twice.plugin(&Notes).unwrap();
        let again = twice.plugin(&Notes).expect_err("claimed twice");
        assert!(format!("{again:#}").contains("claimed twice"), "{again:#}");
    }

    #[test]
    fn a_kind_named_as_another_is_refused() {
        let refused = built(&AppConfig::default(), &Impostor).expect_err("named as the groups are");
        assert!(refused.to_string().contains("impostor"), "names the plugin: {refused}");
        assert!(matches!(refused.root_cause().downcast_ref::<StoreError>(), Some(StoreError::Conflict(_))), "{refused:#}");
    }
}
