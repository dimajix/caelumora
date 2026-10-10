//! Demo data a binary puts into its stores at startup (issue 105): the
//! files `AppConfig::seed` names, each holding preconfigured components,
//! personalities, session templates and skills as the stores keep them
//! (`examples/seed.yaml`). One definition for the REST server and the TUI
//! alike.

use anyhow::Context;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::Personality;
use caelumora_model::component::StoredComponents;
use caelumora_model::skill::Skill;
use caelumora_model::config::parse_yaml;
use caelumora_model::security::{SecurityAttributes, SecurityContext, SecurityManager};
use caelumora_model::store::{ComponentStore, EntityStore, StoreResultExt};
use caelumora_model::template::SessionTemplate;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::info;

/// The documents of one seed file.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Seed {
    /// Preconfigured components (issue 270), which the personalities
    /// include by `kind: ref` - a bundle every agent of a kind shares.
    pub components: Vec<StoredComponents>,
    pub personalities: Vec<SeedPersonality>,
    pub session_templates: Vec<SessionTemplate>,
    /// Skills of the system (issue 22), in the global scope, found by every
    /// agent with the `skills` component.
    pub skills: Vec<Skill>,
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
    /// The personality, its roles resolved by `security`.
    ///
    /// # Errors
    /// If a role it names is not configured.
    fn resolved(&self, security: &dyn SecurityManager) -> anyhow::Result<Personality> {
        let roles = self.roles.as_ref()
            .map(|names| names.iter()
                .map(|name| security.role_id(name).with_context(|| format!("personality {} names the role '{name}', which is not configured", self.personality.name)))
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

    /// Puts every document into `store` - a preconfigured component into
    /// `components` - that it does not have yet, by id - so that seeding
    /// again changes nothing, and a document changed in the store stays as
    /// it is. Stored as the system: the seed belongs to it, every user may
    /// use it and only the system may change it. The roles it names are
    /// resolved by `security` (issue 23).
    ///
    /// # Errors
    /// A role that is not configured; whatever the stores report. What was
    /// stored before stays.
    pub async fn apply(&self, store: &dyn EntityStore, components: &dyn ComponentStore, security: &dyn SecurityManager) -> anyhow::Result<()> {
        let system = SecurityContext::system();
        for component in &self.components {
            if components.get_components(&system, component.id()).await.optional()?.is_none() {
                components.create_components(&system, component).await?;
                info!("Seeded preconfigured component {}", component.id());
            }
        }
        for seeded in &self.personalities {
            let personality = seeded.resolved(security)?;
            if store.get_personality(&system, personality.id).await.optional()?.is_none() {
                store.create_personality(&system, &personality).await?;
                info!("Seeded personality {} ({})", personality.name, personality.id);
            }
        }
        for template in &self.session_templates {
            if store.get_session_template(&system, template.id).await.optional()?.is_none() {
                store.create_session_template(&system, template).await?;
                info!("Seeded session template {} ({})", template.name, template.id);
            }
        }
        for skill in &self.skills {
            if store.get_skill(&system, skill.id).await.optional()?.is_none() {
                // The system's, as everything seeded: a skill says so, since
                // nobody's is no skill at all (`Skill::validate`).
                let owned = Skill { security_attributes: SecurityAttributes::owned_by(ActorId::System), ..skill.clone() };
                store.create_skill(&system, &owned).await?;
                info!("Seeded skill {} ({})", skill.name, skill.id);
            }
        }
        Ok(())
    }
}

/// Reads and applies the seed files `paths`, in order, to `store` and
/// `components`, the roles they name resolved by `security`.
///
/// # Errors
/// As `Seed::read` and `Seed::apply`.
pub async fn load(store: &dyn EntityStore, components: &dyn ComponentStore, security: &dyn SecurityManager, paths: &[PathBuf]) -> anyhow::Result<()> {
    for path in paths {
        Seed::read(path)?.apply(store, components, security).await
            .with_context(|| format!("Cannot seed the stores from {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{ComponentContainer, ComponentContext, ComponentRegistry};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::template::ParticipantTemplate;
    use std::sync::Arc;

    fn example() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/seed.yaml")
    }

    /// The policy of the example configuration, which knows the roles the
    /// example seed names.
    fn policy() -> std::sync::Arc<dyn SecurityManager> {
        let config = caelumora_model::config::AppConfig::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/config.yaml")).unwrap();
        config.create_security_manager(std::sync::Arc::new(caelumora_model::security::NoMemberships))
    }

    #[tokio::test]
    async fn the_example_seed_is_complete_and_seeding_twice_changes_nothing() {
        let seed = Seed::read(&example()).expect("examples/seed.yaml is a seed");
        assert!(!seed.personalities.is_empty() && !seed.session_templates.is_empty());
        let store = MemoryStore::new();
        let system = SecurityContext::system();

        let policy = policy();
        load(&store, &store, policy.as_ref(), &[example()]).await.unwrap();
        // Every personality a template names is part of the seed.
        for template in &seed.session_templates {
            for participant in &template.participants {
                if let ParticipantTemplate::Agent { personality_id, .. } = participant {
                    store.get_personality(&system, *personality_id).await
                        .unwrap_or_else(|_| panic!("template {} names a personality the seed has", template.name));
                }
            }
        }

        // A document changed in the store stays as it is.
        let mut changed = seed.personalities[0].personality.clone();
        changed.description = Some(caelumora_model::value::text::Description::new("changed").unwrap());
        store.update_personality(&system, &changed).await.unwrap();
        load(&store, &store, policy.as_ref(), &[example()]).await.expect("seeding again is no conflict");
        assert_eq!(store.list_personalities(&system).await.unwrap().len(), seed.personalities.len(), "nothing twice");
        assert_eq!(store.list_session_templates(&system).await.unwrap().len(), seed.session_templates.len());
        assert_eq!(store.get_personality(&system, changed.id).await.unwrap().description, changed.description);
        let skills = store.list_skills(&system).await.unwrap();
        assert!(!skills.is_empty() && skills.len() == seed.skills.len(), "the skills, once (issue 22)");
        assert!(skills.iter().all(|s| s.security_attributes.owner_id == ActorId::System), "the system's");
        let components = store.list_components(&system).await.unwrap();
        assert!(!components.is_empty() && components.len() == seed.components.len(), "the preconfigured components, once (issue 270)");
    }

    /// Every personality of the seed can be built with what the seed
    /// brings: the components it includes are there, and none of them is
    /// written twice (issue 270).
    #[tokio::test]
    async fn every_personality_of_the_example_seed_can_be_built() {
        let seed = Seed::read(&example()).expect("examples/seed.yaml is a seed");
        let store = Arc::new(MemoryStore::new());
        load(store.as_ref(), store.as_ref(), policy().as_ref(), &[example()]).await.unwrap();
        let registry = Arc::new(ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store)));
        let security = Arc::new(DummySecurityManager::new());
        let context = ComponentContext::new(SecurityContext::system(), Arc::<DummySecurityManager>::clone(&security), Arc::new(SecretManager::new(vec![], security)));
        for personality in seed.personalities.iter().map(|seeded| &seeded.personality) {
            ComponentContainer::from_config(&personality.components, &registry, &context).await
                .unwrap_or_else(|e| panic!("personality {} cannot be built: {e}", personality.name));
        }
    }

    #[tokio::test]
    async fn a_seed_names_roles_which_are_stored_by_their_ids() {
        let seed: Seed = parse_yaml("personalities:\n- id: ff9a5e56-e5ea-43f9-9114-2098a6f417a9\n  name: limited\n  roles: [staff]\n").unwrap();
        let (store, policy) = (MemoryStore::new(), policy());
        seed.apply(&store, &store, policy.as_ref()).await.unwrap();
        let stored = store.list_personalities(&SecurityContext::system()).await.unwrap();
        assert_eq!(stored[0].roles, Some(vec![policy.role_id("staff").unwrap()]), "by the id the configuration gives it");
        let unknown: Seed = parse_yaml("personalities:\n- id: 1f9a5e56-e5ea-43f9-9114-2098a6f417a9\n  name: typo\n  roles: [staf]\n").unwrap();
        let error = unknown.apply(&store, &store, policy.as_ref()).await.expect_err("a role not configured");
        assert!(error.to_string().contains("staf"), "{error}");
    }

    #[test]
    fn a_seed_with_an_unknown_key_is_refused() {
        let error = parse_yaml::<Seed>("personalites: []\n").expect_err("a typo");
        assert!(error.to_string().contains("unknown field"), "{error}");
    }
}
