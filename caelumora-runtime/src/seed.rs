//! Demo data a binary puts into its entity store at startup (issue 105):
//! the files `AppConfig::seed` names, each holding personalities and
//! session templates as the store keeps them (`examples/seed.yaml`). One
//! definition for the REST server and the TUI alike.

use anyhow::Context;
use caelumora_model::agent::Personality;
use caelumora_model::config::parse_yaml;
use caelumora_model::security::SecurityContext;
use caelumora_model::store::{EntityStore, StoreResultExt};
use caelumora_model::template::SessionTemplate;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::info;

/// The documents of one seed file.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Seed {
    pub personalities: Vec<Personality>,
    pub session_templates: Vec<SessionTemplate>,
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

    /// Puts every document into `store` that it does not have yet, by id -
    /// so that seeding again changes nothing, and a document changed in the
    /// store stays as it is. Stored as the system: the seed belongs to it,
    /// every user may use it and only the system may change it.
    ///
    /// # Errors
    /// Whatever the store reports; what was stored before stays.
    pub async fn apply(&self, store: &dyn EntityStore) -> anyhow::Result<()> {
        let system = SecurityContext::system();
        for personality in &self.personalities {
            if store.get_personality(&system, personality.id).await.optional()?.is_none() {
                store.create_personality(&system, personality).await?;
                info!("Seeded personality {} ({})", personality.name, personality.id);
            }
        }
        for template in &self.session_templates {
            if store.get_session_template(&system, template.id).await.optional()?.is_none() {
                store.create_session_template(&system, template).await?;
                info!("Seeded session template {} ({})", template.name, template.id);
            }
        }
        Ok(())
    }
}

/// Reads and applies the seed files `paths`, in order.
///
/// # Errors
/// As `Seed::read` and `Seed::apply`.
pub async fn load(store: &dyn EntityStore, paths: &[PathBuf]) -> anyhow::Result<()> {
    for path in paths {
        Seed::read(path)?.apply(store).await
            .with_context(|| format!("Cannot seed the entity store from {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::template::ParticipantTemplate;

    fn example() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/seed.yaml")
    }

    #[tokio::test]
    async fn the_example_seed_is_complete_and_seeding_twice_changes_nothing() {
        let seed = Seed::read(&example()).expect("examples/seed.yaml is a seed");
        assert!(!seed.personalities.is_empty() && !seed.session_templates.is_empty());
        let store = MemoryStore::new();
        let system = SecurityContext::system();

        load(&store, &[example()]).await.unwrap();
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
        let mut changed = seed.personalities[0].clone();
        changed.description = Some(caelumora_model::value::text::Description::new("changed").unwrap());
        store.update_personality(&system, &changed).await.unwrap();
        load(&store, &[example()]).await.expect("seeding again is no conflict");
        assert_eq!(store.list_personalities(&system).await.unwrap().len(), seed.personalities.len(), "nothing twice");
        assert_eq!(store.list_session_templates(&system).await.unwrap().len(), seed.session_templates.len());
        assert_eq!(store.get_personality(&system, changed.id).await.unwrap().description, changed.description);
    }

    #[test]
    fn a_seed_with_an_unknown_key_is_refused() {
        let error = parse_yaml::<Seed>("personalites: []\n").expect_err("a typo");
        assert!(error.to_string().contains("unknown field"), "{error}");
    }
}
