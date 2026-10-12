//! Demo data a binary puts into its stores at startup (issue 105): the
//! files `AppConfig::seed` names, each holding preconfigured components,
//! personalities, session templates and skills as the stores keep them
//! (`examples/seed.yaml`), and the sections of the plugins (issue 323).
//! One definition for the REST server and the TUI alike
//! (`RuntimeManager::seed`).

use anyhow::{bail, Context};
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::Personality;
use caelumora_model::component::StoredComponents;
use caelumora_model::skill::Skill;
use caelumora_model::config::parse_yaml;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use caelumora_model::security::{SecurityAttributes, SecurityContext};
use caelumora_model::store::{ComponentStore, EntityStores, StoreResultExt};
use caelumora_model::template::SessionTemplate;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

/// The documents of one seed file. A key it does not know is a section of
/// a plugin's; one that no plugin claims is an error when it is seeded.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Seed {
    /// Preconfigured components (issue 270), which the personalities
    /// include by `kind: ref` - a bundle every agent of a kind shares.
    pub components: Vec<StoredComponents>,
    pub personalities: Vec<SeedPersonality>,
    pub session_templates: Vec<SessionTemplate>,
    /// Skills of the system (issue 22), in the global scope, found by every
    /// agent with the `skills` component.
    pub skills: Vec<Skill>,
    /// The sections of the plugins, by name, as they were written: each
    /// seeded by the plugin that claims it (`SeedSection`, issue 323).
    #[serde(flatten)]
    pub plugins: BTreeMap<String, serde_json::Value>,
}

/// A section of the seed files a plugin claims (issue 323,
/// `RuntimeBuilder::seed_section`): what it seeds of its kinds.
#[async_trait]
pub trait SeedSection: Debug + Send + Sync {
    /// Puts what `section` holds into the plugin's stores as the system,
    /// each document it does not have yet, by id - so that seeding again
    /// changes nothing, and a document changed in the store stays as it is.
    ///
    /// # Errors
    /// If `section` is not what the plugin seeds; whatever the stores
    /// report. What was stored before stays.
    async fn seed(&self, section: serde_json::Value) -> anyhow::Result<()>;
}

/// A personality as a seed writes it: as the store keeps it, but with the
/// roles its agents are limited to named by their names - written by hand
/// (issue 23) -, resolved to their ids when it is applied.
#[derive(Debug, Clone, Deserialize)]
pub struct SeedPersonality {
    #[serde(default)]
    roles: Option<Vec<String>>,
    #[serde(flatten)]
    pub personality: Personality,
}

impl SeedPersonality {
    /// The personality, its roles resolved by the roles `confidentiality`
    /// is configured with.
    ///
    /// # Errors
    /// If a role it names is not configured.
    fn resolved(&self, confidentiality: &dyn ConfidentialityPolicy) -> anyhow::Result<Personality> {
        let roles = self.roles.as_ref()
            .map(|names| names.iter()
                .map(|name| confidentiality.roles().and_then(|roles| roles.id_of(name).ok())
                    .with_context(|| format!("personality {} names the role '{name}', which is not configured", self.personality.name)))
                .collect::<anyhow::Result<Vec<_>>>())
            .transpose()?;
        Ok(Personality { roles, ..self.personality.clone() })
    }
}

impl Seed {
    /// Reads the seed file at `path`.
    ///
    /// # Errors
    /// If the file cannot be read or is no seed; the error names the file
    /// and, for a malformed one, the line.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read the seed file {}", path.display()))?;
        parse_yaml(&content).with_context(|| format!("Cannot parse the seed file {}", path.display()))
    }

    /// Puts every document into `stores` - a preconfigured component into
    /// `components` - that it does not have yet, by id - so that seeding
    /// again changes nothing, and a document changed in the store stays as
    /// it is. Stored as the system: the seed belongs to it, every user may
    /// use it and only the system may change it. The roles it names are
    /// resolved by the roles `confidentiality` is configured with (issue 23).
    /// The sections of the plugins are seeded last, each by the
    /// `SeedSection` of `sections` that claims it.
    ///
    /// # Errors
    /// A section no plugin claims, before anything is stored; a role that
    /// is not configured; whatever the stores report. What was stored
    /// before stays.
    pub async fn apply(&self, stores: &EntityStores, components: &dyn ComponentStore, confidentiality: &dyn ConfidentialityPolicy, sections: &BTreeMap<String, Arc<dyn SeedSection>>) -> anyhow::Result<()> {
        if let Some(name) = self.plugins.keys().find(|name| !sections.contains_key(*name)) {
            bail!("The seed has a section '{name}' that no plugin claims");
        }
        let system = SecurityContext::system();
        for component in &self.components {
            if components.get_components(&system, component.id()).await.optional()?.is_none() {
                components.create_components(&system, component).await?;
                info!("Seeded preconfigured component {}", component.id());
            }
        }
        for seeded in &self.personalities {
            let personality = seeded.resolved(confidentiality)?;
            if stores.personalities.get(&system, personality.id).await.optional()?.is_none() {
                stores.personalities.create(&system, &personality).await?;
                info!("Seeded personality {} ({})", personality.name, personality.id);
            }
        }
        for template in &self.session_templates {
            if stores.session_templates.get(&system, template.id).await.optional()?.is_none() {
                stores.session_templates.create(&system, template).await?;
                info!("Seeded session template {} ({})", template.name, template.id);
            }
        }
        for skill in &self.skills {
            if stores.skills.get(&system, skill.id).await.optional()?.is_none() {
                // The system's, as everything seeded: a skill says so, since
                // nobody's is no skill at all (`Skill::validate`).
                let owned = Skill { security_attributes: SecurityAttributes::owned_by(ActorId::System), ..skill.clone() };
                stores.skills.create(&system, &owned).await?;
                info!("Seeded skill {} ({})", skill.name, skill.id);
            }
        }
        for (name, section) in &self.plugins {
            if let Some(seeder) = sections.get(name) {
                seeder.seed(section.clone()).await.with_context(|| format!("Cannot seed the section '{name}'"))?;
            }
        }
        Ok(())
    }
}

/// Reads and applies the seed files `paths`, in order, to `stores` and
/// `components` and through the plugins' `sections`, the roles they name
/// resolved by `confidentiality`.
///
/// # Errors
/// As `Seed::read` and `Seed::apply`.
pub async fn load(stores: &EntityStores, components: &dyn ComponentStore, confidentiality: &dyn ConfidentialityPolicy, sections: &BTreeMap<String, Arc<dyn SeedSection>>, paths: &[PathBuf]) -> anyhow::Result<()> {
    for path in paths {
        Seed::read(path)?.apply(stores, components, confidentiality, sections).await
            .with_context(|| format!("Cannot seed the stores from {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use caelumora_model::store::query::Query;
    use crate::component::{ComponentContainer, ComponentContext, ComponentRegistry};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::ComponentStore;
    use caelumora_model::template::ParticipantTemplate;
    use std::sync::Arc;
    use caelumora_model::config::AppConfig;
    use crate::runtime::RuntimeManager;

    fn example() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/seed.yaml")
    }

    /// The policy of the example configuration, which knows the roles the
    /// example seed names.
    fn policy() -> std::sync::Arc<dyn ConfidentialityPolicy> {
        let config = AppConfig::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/config.yaml")).unwrap();
        config.create_confidentiality_policy()
    }

    #[tokio::test]
    async fn the_example_seed_is_complete_and_seeding_twice_changes_nothing() {
        let seed = Seed::read(&example()).expect("examples/seed.yaml is a seed");
        assert!(!seed.personalities.is_empty() && !seed.session_templates.is_empty());
        let store = Arc::new(MemoryStore::new());
        let stores = store.entity_stores();
        let system = SecurityContext::system();

        let policy = policy();
        load(&stores, store.as_ref(), policy.as_ref(), &BTreeMap::new(), &[example()]).await.unwrap();
        // Every personality a template names is part of the seed.
        for template in &seed.session_templates {
            for participant in &template.participants {
                if let ParticipantTemplate::Agent { personality_id, .. } = participant {
                    store.entity_stores().personalities.get(&system, *personality_id).await
                        .unwrap_or_else(|_| panic!("template {} names a personality the seed has", template.name));
                }
            }
        }

        // A document changed in the store stays as it is.
        let mut changed = seed.personalities[0].personality.clone();
        changed.description = Some(caelumora_model::value::text::Description::new("changed").unwrap());
        store.entity_stores().personalities.update(&system, &changed).await.unwrap();
        load(&stores, store.as_ref(), policy.as_ref(), &BTreeMap::new(), &[example()]).await.expect("seeding again is no conflict");
        assert_eq!(store.entity_stores().personalities.list(&system, &Query::all()).await.map(|page| page.items).unwrap().len(), seed.personalities.len(), "nothing twice");
        assert_eq!(stores.session_templates.list(&system, &Query::all()).await.map(|page| page.items).unwrap().len(), seed.session_templates.len());
        assert_eq!(store.entity_stores().personalities.get(&system, changed.id).await.unwrap().description, changed.description);
        let skills = stores.skills.list(&system, &Query::all()).await.map(|page| page.items).unwrap();
        assert!(!skills.is_empty() && skills.len() == seed.skills.len(), "the skills, once (issue 22)");
        assert!(skills.iter().all(|s| s.security_attributes.owner_id == ActorId::System), "the system's");
        let components = store.list_components(&system, &Query::all()).await.map(|page| page.items).unwrap();
        assert!(!components.is_empty() && components.len() == seed.components.len(), "the preconfigured components, once (issue 270)");
    }

    /// Every personality of the seed can be built with what the seed
    /// brings: the components it includes are there, and none of them is
    /// written twice (issue 270).
    #[tokio::test]
    async fn every_personality_of_the_example_seed_can_be_built() {
        let seed = Seed::read(&example()).expect("examples/seed.yaml is a seed");
        let store = Arc::new(MemoryStore::new());
        let confidentiality = policy();
        load(&store.entity_stores(), store.as_ref(), confidentiality.as_ref(), &BTreeMap::new(), &[example()]).await.unwrap();
        let security = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&security)));
        // The services a runtime gives them (issue 296).
        let runtime = RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, security, Arc::clone(&confidentiality), &Default::default(), ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap();
        let context = ComponentContext::new(SecurityContext::system(), runtime.services().clone());
        for personality in seed.personalities.iter().map(|seeded| &seeded.personality) {
            ComponentContainer::from_config(&personality.components, runtime.component_registry(), &context).await
                .unwrap_or_else(|e| panic!("personality {} cannot be built: {e}", personality.name));
        }
    }

    #[tokio::test]
    async fn a_seed_names_roles_which_are_stored_by_their_ids() {
        let seed: Seed = parse_yaml("personalities:\n- id: ff9a5e56-e5ea-43f9-9114-2098a6f417a9\n  name: limited\n  roles: [staff]\n").unwrap();
        let (store, policy) = (Arc::new(MemoryStore::new()), policy());
        seed.apply(&store.entity_stores(), store.as_ref(), policy.as_ref(), &BTreeMap::new()).await.unwrap();
        let stored = store.entity_stores().personalities.list(&SecurityContext::system(), &Query::all()).await.map(|page| page.items).unwrap();
        assert_eq!(stored[0].roles, Some(vec![policy.roles().unwrap().id_of("staff").unwrap()]), "by the id the configuration gives it");
        let unknown: Seed = parse_yaml("personalities:\n- id: 1f9a5e56-e5ea-43f9-9114-2098a6f417a9\n  name: typo\n  roles: [staf]\n").unwrap();
        let error = unknown.apply(&store.entity_stores(), store.as_ref(), policy.as_ref(), &BTreeMap::new()).await.expect_err("a role not configured");
        assert!(error.to_string().contains("staf"), "{error}");
    }

    /// What a plugin was given of its section.
    #[derive(Debug, Default)]
    struct Given(Mutex<Vec<serde_json::Value>>);

    #[async_trait]
    impl SeedSection for Given {
        async fn seed(&self, section: serde_json::Value) -> anyhow::Result<()> {
            self.0.lock().unwrap().push(section);
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_plugins_section_is_seeded_by_it_and_one_nobody_claims_is_refused_before_anything_is_stored() {
        let (store, policy) = (Arc::new(MemoryStore::new()), policy());
        let given = Arc::new(Given::default());
        let sections: BTreeMap<String, Arc<dyn SeedSection>> = [("notes".to_string(), Arc::<Given>::clone(&given) as Arc<dyn SeedSection>)].into();
        let seed: Seed = parse_yaml("notes:\n- text: remember\n").unwrap();
        seed.apply(&store.entity_stores(), store.as_ref(), policy.as_ref(), &sections).await.unwrap();
        assert_eq!(*given.0.lock().unwrap(), [serde_json::json!([{"text": "remember"}])]);

        let typo: Seed = parse_yaml("personalites: []\npersonalities:\n- id: 2f9a5e56-e5ea-43f9-9114-2098a6f417a9\n  name: kept-out\n").unwrap();
        let error = typo.apply(&store.entity_stores(), store.as_ref(), policy.as_ref(), &sections).await.expect_err("a typo");
        assert!(error.to_string().contains("'personalites'"), "{error}");
        let stored = store.entity_stores().personalities.list(&SecurityContext::system(), &Query::all()).await.map(|page| page.items).unwrap();
        assert!(stored.is_empty(), "nothing stored");
    }
}
