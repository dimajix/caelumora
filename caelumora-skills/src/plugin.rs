//! How skills are joined to the runtime (issue 323): the store of their
//! kind, the `SkillManager` as a service - what the component, the views
//! and the routes reach it by - and as a member of a scope, the `skills`
//! component, and the section `skills` of the seed files.

use crate::component;
use crate::manager::SkillManager;
use crate::skill::Skill;
use crate::{NAMESPACE, PROMPTS};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::store::EntityStore;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use caelumora_runtime::seed::{create_unless_taken, SeedSection};
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use tracing::info;

/// The section of the seed files and the name of the plugin.
pub const SKILLS: &str = "skills";

/// Skills, as a plugin.
#[derive(Debug, Default)]
pub struct SkillsPlugin;

impl Plugin for SkillsPlugin {
    fn name(&self) -> &'static str {
        SKILLS
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        let skills = runtime.entity_store::<Skill>()?;
        let manager = Arc::new(SkillManager::new(
            Arc::clone(&skills),
            Arc::clone(runtime.scope_holders()),
            Arc::clone(runtime.event_bus()),
            Arc::clone(runtime.confidentiality()),
        ));
        runtime.provide(Arc::clone(&manager));
        runtime.scope_member(manager);
        runtime.seed_section(SKILLS, Arc::new(SeededSkills(skills)))?;
        runtime.prompts().add(NAMESPACE, PROMPTS)?;
        component::register(runtime.components());
        Ok(())
    }
}

/// The skills of the system a seed file holds (issue 22), in the global
/// scope, found by every agent with the `skills` component.
struct SeededSkills(Arc<dyn EntityStore<Skill>>);

impl fmt::Debug for SeededSkills {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeededSkills").finish_non_exhaustive()
    }
}

#[async_trait]
impl SeedSection for SeededSkills {
    async fn seed(&self, section: Value) -> Result<()> {
        for skill in serde_json::from_value::<Vec<Skill>>(section)? {
            if create_unless_taken(self.0.as_ref(), &skill).await? {
                info!("Seeded skill {} ({})", skill.name, skill.id);
            }
        }
        Ok(())
    }
}
