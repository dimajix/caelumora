//! Demo data a binary puts into its stores at startup (issue 105): the
//! files `AppConfig::seed` names, each holding preconfigured components,
//! personalities and session templates as the stores keep them
//! (`examples/seed.yaml`), and the sections of the plugins - the skills
//! among them (issue 323).
//! One definition for the REST server and the TUI alike
//! (`RuntimeManager::seed`).

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use caelumora_model::agent::Personality;
use caelumora_model::component::StoredComponents;
use caelumora_model::config::parse_yaml;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use caelumora_model::security::SecurityContext;
use caelumora_model::store::{ComponentStore, EntityStores, StoreResultExt};
use caelumora_model::template::SessionTemplate;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;
use serde_json::Value;
use std::fs;

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
    /// The sections of the plugins, by name, as they were written: each
    /// seeded by the plugin that claims it (`SeedSection`, issue 323).
    #[serde(flatten)]
    pub plugins: BTreeMap<String, Value>,
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
    async fn seed(&self, section: Value) -> Result<()>;
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
    fn resolved(&self, confidentiality: &dyn ConfidentialityPolicy) -> Result<Personality> {
        let roles = self.roles.as_ref()
            .map(|names| names.iter()
                .map(|name| confidentiality.roles().and_then(|roles| roles.id_of(name).ok())
                    .with_context(|| format!("personality {} names the role '{name}', which is not configured", self.personality.name)))
                .collect::<Result<Vec<_>>>())
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
    pub fn read(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
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
    pub async fn apply(&self, stores: &EntityStores, components: &dyn ComponentStore, confidentiality: &dyn ConfidentialityPolicy, sections: &BTreeMap<String, Arc<dyn SeedSection>>) -> Result<()> {
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
pub async fn load(stores: &EntityStores, components: &dyn ComponentStore, confidentiality: &dyn ConfidentialityPolicy, sections: &BTreeMap<String, Arc<dyn SeedSection>>, paths: &[PathBuf]) -> Result<()> {
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
    
    
    
    use caelumora_model::store::Persistence;
    
    
    use caelumora_model::config::AppConfig;
    

    /// The policy of the example configuration, which knows the roles the
    /// example seed names.
    fn policy() -> Arc<dyn ConfidentialityPolicy> {
        let config = AppConfig::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/config.yaml")).unwrap();
        config.create_confidentiality_policy()
    }

    #[tokio::test]
    async fn a_seed_names_roles_which_are_stored_by_their_ids() {
        let seed: Seed = parse_yaml("personalities:\n- id: ff9a5e56-e5ea-43f9-9114-2098a6f417a9\n  name: limited\n  roles: [staff]\n").unwrap();
        let (store, policy) = (Persistence::in_memory(), policy());
        seed.apply(&store.entity_stores().unwrap(), store.component_store().unwrap().as_ref(), policy.as_ref(), &BTreeMap::new()).await.unwrap();
        let stored = store.entity_stores().unwrap().personalities.list(&SecurityContext::system(), &Query::all()).await.map(|page| page.items).unwrap();
        assert_eq!(stored[0].roles, Some(vec![policy.roles().unwrap().id_of("staff").unwrap()]), "by the id the configuration gives it");
        let unknown: Seed = parse_yaml("personalities:\n- id: 1f9a5e56-e5ea-43f9-9114-2098a6f417a9\n  name: typo\n  roles: [staf]\n").unwrap();
        let error = unknown.apply(&store.entity_stores().unwrap(), store.component_store().unwrap().as_ref(), policy.as_ref(), &BTreeMap::new()).await.expect_err("a role not configured");
        assert!(error.to_string().contains("staf"), "{error}");
    }

    /// What a plugin was given of its section.
    #[derive(Debug, Default)]
    struct Given(Mutex<Vec<Value>>);

    #[async_trait]
    impl SeedSection for Given {
        async fn seed(&self, section: Value) -> Result<()> {
            self.0.lock().unwrap().push(section);
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_plugins_section_is_seeded_by_it_and_one_nobody_claims_is_refused_before_anything_is_stored() {
        let (store, policy) = (Persistence::in_memory(), policy());
        let given = Arc::new(Given::default());
        let sections: BTreeMap<String, Arc<dyn SeedSection>> = [("notes".to_string(), Arc::<Given>::clone(&given) as Arc<dyn SeedSection>)].into();
        let seed: Seed = parse_yaml("notes:\n- text: remember\n").unwrap();
        seed.apply(&store.entity_stores().unwrap(), store.component_store().unwrap().as_ref(), policy.as_ref(), &sections).await.unwrap();
        assert_eq!(*given.0.lock().unwrap(), [serde_json::json!([{"text": "remember"}])]);

        let typo: Seed = parse_yaml("personalites: []\npersonalities:\n- id: 2f9a5e56-e5ea-43f9-9114-2098a6f417a9\n  name: kept-out\n").unwrap();
        let error = typo.apply(&store.entity_stores().unwrap(), store.component_store().unwrap().as_ref(), policy.as_ref(), &sections).await.expect_err("a typo");
        assert!(error.to_string().contains("'personalites'"), "{error}");
        let stored = store.entity_stores().unwrap().personalities.list(&SecurityContext::system(), &Query::all()).await.map(|page| page.items).unwrap();
        assert!(stored.is_empty(), "nothing stored");
    }
}
