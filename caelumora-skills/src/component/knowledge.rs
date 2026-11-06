//! The facet `KnowledgeRestorer` of `skills` (issue 323): the skills loaded in what a compaction
//! summarises, given again as they are now - where the agent may still load them.

use super::{reader_in, SkillsComponent};
use crate::skill::Skill;
use async_trait::async_trait;
use caelumora_agents::turn::Turn;
use caelumora_compaction::{CompactionError, GivenAgain, KnowledgeRestorer};
use caelumora_domain::context::KnowledgeSource;
use caelumora_domain::store::{EntityKind, Stored};
use std::fmt::Display;
use tracing::warn;

#[async_trait]
impl KnowledgeRestorer for SkillsComponent {
    fn kind(&self) -> EntityKind {
        Skill::KIND
    }

    /// The skills of `given` the agent may still load where it is.
    async fn give_again(
        &self,
        turn: &mut Turn<'_>,
        given: &[KnowledgeSource],
    ) -> Result<Vec<GivenAgain>, CompactionError> {
        let agent = turn.situation().agent_id();
        let unreadable = |e: &dyn Display| {
            warn!("Agent {}: the skills it had loaded could not be read again: {:#}", agent, e);
            CompactionError::KnowledgeUnreadable
        };
        let skills = reader_in(&turn.situation()).await.ok_or(CompactionError::NoSession)?;
        let usable = skills.usable_skills().await.map_err(|e| unreadable(&e))?;
        given
            .iter()
            .filter_map(|known| known.about::<Skill>())
            .filter_map(|(id, _)| usable.iter().find(|skill| skill.id == id))
            .map(|skill| {
                Ok(GivenAgain {
                    name: skill.name.name.to_string(),
                    text: self.given(skill)?,
                    knowledge: skill
                        .knowledge(turn.situation().channel_id(), turn.situation().session_id()),
                })
            })
            .collect()
    }
}
