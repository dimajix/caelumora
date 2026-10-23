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
use anyhow::Result;

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
    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{ComponentContainer, ComponentContext, ComponentFacets, ComponentKind};
    use crate::testing::TestRuntime;
    use crate::runtime::RuntimeManager;
    use crate::seed::SeedSection;
    use crate::session::SessionManager;
    use crate::view::Viewer;
    use async_trait::async_trait;
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::config::AppConfig;
    use caelumora_model::define_id;
    use caelumora_model::group::Group;
    use caelumora_model::removal::{Deletable, Removal};
    use caelumora_model::secret::{SecretManager, SecretProvider, SecretResult};
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::{PersistenceBackend, StateStore, StoreResult};
    use caelumora_model::store::documents::{DocumentOf, DocumentStorage};
    use caelumora_model::store::history::HistoryStore;
    use caelumora_model::store::transcript::TranscriptStore;
    use crate::component::{config_of_params, service_of};
    use caelumora_model::security::{Secured, SecurityAttributes, SecurityContext};
    use caelumora_model::store::Persistence;
    use caelumora_model::store::query::NoField;
    use caelumora_model::store::{EntityKind, EntityStore, EntityStoreFactory, NoEntity, StoreError, Stored};
    use caelumora_model::type_map::TypeKey;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use serde_json::Value;
    use std::fs;

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
        async fn seed(&self, section: Value) -> Result<()> {
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

        fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
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

        fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
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

        fn register(&self, _runtime: &mut RuntimeBuilder) -> Result<()> {
            Ok(())
        }
    }

    /// The runtime with `plugin` alone, configured as `config` says, or why it could not be built.
    fn built(config: AppConfig, plugin: &dyn Plugin) -> Result<RuntimeManager> {
        TestRuntime { config, plugins: vec![plugin], ..Default::default() }.try_build()
    }

    #[tokio::test]
    async fn a_plugin_brings_its_kind_its_manager_its_component_and_its_sections() {
        let store = Persistence::in_memory();
        let config = AppConfig::from_yaml("notes:\n  greeting: hello\n").unwrap();
        let runtime = TestRuntime { store: store.clone(), config, plugins: vec![&Notes], ..Default::default() }.build();

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
        fs::write(&seed, "notes:\n- id: 3f9a5e56-e5ea-43f9-9114-2098a6f417a9\n  text: remember\n").unwrap();
        runtime.seed(&[seed]).await.unwrap();
        let notes = store.entity_store::<Note>().unwrap();
        let note = notes.get(&SecurityContext::system(), "3f9a5e56-e5ea-43f9-9114-2098a6f417a9".parse().unwrap()).await.expect("seeded");
        assert_eq!(note.text, "remember");
    }

    /// Brings a backend of the stores, `probe`, kept in memory, that can be
    /// made only with the secrets at hand - as a database that connects by
    /// one.
    struct Probe;

    impl Plugin for Probe {
        fn name(&self) -> &'static str {
            "probe"
        }

        fn backends(&self, backends: &mut Backends) {
            backends.register::<dyn PersistenceBackend, _>("probe", |_, _, services| {
                service_of::<SecretManager>(services)?;
                Ok(Arc::new(MemoryStore::new()))
            });
        }

        fn register(&self, _runtime: &mut RuntimeBuilder) -> Result<()> {
            Ok(())
        }
    }

    /// The backends of the stores are made with the secrets at hand (issue
    /// 447), which are made before them.
    #[tokio::test]
    async fn a_backend_of_the_stores_is_given_the_secrets() {
        let config = AppConfig::from_yaml("persistence:\n  entity_store:\n    type: probe\n  state_store:\n    type: probe\n").unwrap();
        RuntimeBuilder::from_config(&config, &[&Probe]).await.expect("made with the secrets").build().unwrap();
    }

    /// Brings a backend of the stores, `ledger`, that keeps the entities
    /// alone - as a database of the entities, before it keeps the states.
    struct Ledger;

    #[derive(Debug)]
    struct EntitiesAlone(MemoryStore);

    impl PersistenceBackend for EntitiesAlone {
        fn documents(&self, kind: EntityKind, document_of: DocumentOf) -> StoreResult<Arc<dyn DocumentStorage>> {
            self.0.documents(kind, document_of)
        }

        fn state_store(self: Arc<Self>) -> StoreResult<Arc<dyn StateStore>> {
            Err(StoreError::Backend("the ledger keeps no states".to_string()))
        }

        fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>> {
            Err(StoreError::Backend("the ledger keeps no histories".to_string()))
        }

        fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>> {
            Err(StoreError::Backend("the ledger keeps no transcripts".to_string()))
        }
    }

    impl Plugin for Ledger {
        fn name(&self) -> &'static str {
            "ledger"
        }

        fn backends(&self, backends: &mut Backends) {
            backends.register::<dyn PersistenceBackend, _>("ledger", |_, _, _| Ok(Arc::new(EntitiesAlone(MemoryStore::new()))));
        }

        fn register(&self, _runtime: &mut RuntimeBuilder) -> Result<()> {
            Ok(())
        }
    }

    /// A backend that keeps the entities alone is the entity store beside
    /// another of the states (issue 448), and fails the start as the state
    /// store, naming it.
    #[tokio::test]
    async fn a_backend_of_the_entities_alone_is_no_state_store() {
        let beside = AppConfig::from_yaml("persistence:\n  entity_store:\n    type: ledger\n  state_store:\n    type: memory\n").unwrap();
        RuntimeBuilder::from_config(&beside, &[&Ledger]).await.expect("the states kept in memory").build().unwrap();
        let alone = AppConfig::from_yaml("persistence:\n  entity_store:\n    type: ledger\n  state_store:\n    type: ledger\n").unwrap();
        let refused = RuntimeBuilder::from_config(&alone, &[&Ledger]).await.map(|_| ()).expect_err("it keeps no states");
        assert!(format!("{refused:#}").contains("Cannot make the state store: backend: the ledger keeps no states"), "{refused:#}");
    }

    #[tokio::test]
    async fn a_plugin_brings_a_backend_of_a_service_of_the_core() {
        let config = AppConfig::from_yaml("secrets:\n  providers:\n    - type: vault\n      secret: s3cr3t\n").unwrap();
        let runtime = RuntimeBuilder::from_config(&config, &[&Vault]).await.unwrap().build().unwrap();
        assert_eq!(runtime.secret_manager().resolve(&SecurityContext::system(), "ANY").await.unwrap(), "s3cr3t", "made by the plugin's factory");
        let without = RuntimeBuilder::from_config(&config, &[]).await.map(|_| ()).expect_err("nobody brings it");
        assert!(format!("{without:#}").contains("the secret provider 1 (vault)"), "{without:#}");
    }

    #[test]
    fn a_section_of_the_configuration_nobody_claims_is_an_error() {
        let refused = built(AppConfig::from_yaml("notez:\n  greeting: hello\n").unwrap(), &Notes).expect_err("a typo");
        assert!(refused.to_string().contains("'notez'"), "{refused}");
        let unreadable = built(AppConfig::from_yaml("notes:\n  greting: hello\n").unwrap(), &Notes).expect_err("not its section");
        assert!(format!("{unreadable:#}").contains("greting"), "{unreadable:#}");
        let again = TestRuntime { plugins: vec![&Notes, &Notes], ..Default::default() }.try_build().expect_err("claimed twice");
        assert!(format!("{again:#}").contains("claimed twice"), "{again:#}");
    }

    #[test]
    fn a_kind_named_as_another_is_refused() {
        let refused = built(AppConfig::default(), &Impostor).expect_err("named as the groups are");
        assert!(refused.to_string().contains("impostor"), "names the plugin: {refused}");
        assert!(matches!(refused.root_cause().downcast_ref::<StoreError>(), Some(StoreError::Conflict(_))), "{refused:#}");
    }
}
