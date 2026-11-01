//! How skills are joined to the kernel (issue 323): the store of their kind, the `SkillManager` as
//! a service - what the views and the routes reach it by - and as a member of a scope, the
//! `SkillReader` and the `SkillWriter` bound to the views of an agent's work - what the component
//! reaches skills by (issue 516) -, and the section `skills` of the seed files.

use crate::extension::{SkillReader, SkillWriter};
use crate::manager::SkillManager;
use crate::skill::Skill;
use crate::SKILLS;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use caelumora_kernel::seed::{create_unless_taken, SeedSection};
use caelumora_model::store::EntityStore;
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use tracing::info;

/// Skills, as a plugin.
#[derive(Debug, Default)]
pub struct SkillsPlugin;

impl Plugin for SkillsPlugin {
    fn name(&self) -> &'static str {
        SKILLS
    }

    fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
        let skills = kernel.entity_store::<Skill>()?;
        let manager = Arc::new(SkillManager::new(
            Arc::clone(&skills),
            Arc::clone(kernel.scope_holders()),
            Arc::clone(kernel.event_bus()),
            Arc::clone(kernel.confidentiality()),
        ));
        kernel.provide(Arc::clone(&manager));
        kernel.scope_member(manager);
        kernel.extend_readers::<dyn SkillReader, _, _>(|session| async move {
            Some(Arc::new(session.views().clone()) as Arc<dyn SkillReader>)
        })?;
        kernel.extend_writers::<dyn SkillWriter, _, _>(|session| async move {
            Some(Arc::new(session.views().clone()) as Arc<dyn SkillWriter>)
        })?;
        kernel.seed_section(SKILLS, Arc::new(SeededSkills(skills)))?;
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
        for mut skill in serde_json::from_value::<Vec<Skill>>(section)? {
            if create_unless_taken(self.0.as_ref(), &mut skill).await? {
                info!("Seeded skill {} ({})", skill.name, skill.id);
            }
        }
        Ok(())
    }
}
