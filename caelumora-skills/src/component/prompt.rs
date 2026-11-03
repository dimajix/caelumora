//! The facet `PromptProvider` of `skills` (issue 22): the catalogue - name and description of every
//! skill the agent may load where it is (`SkillReader::usable_skills`), in the system prompt by
//! default (parameter `position`) - it changes rarely, and the model has to know what it could
//! load. Classified as the join of the skills it lists.

use super::{joined, reader_in, SkillsComponent, LOAD_SKILL_TOOL_NAME};
use crate::extension::SkillError;
use crate::skill::Skill;
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::prompt::{ContextError, Prompt, PromptPosition, PromptProvider};
use caelumora_agents::situation::Situation;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{context, PromptError};

impl SkillsComponent {
    /// The catalogue of `skills`: a line each, its name and what it is for;
    /// empty if there are none.
    fn catalogue(&self, skills: &[Skill]) -> Result<String, PromptError> {
        if skills.is_empty() {
            return Ok(String::new());
        }
        let skills: Vec<_> = skills.iter().map(|skill| context! { name => skill.name.name.to_string(), description => skill.description.to_string() }).collect();
        self.prompts.render(
            &format!("{NAMESPACE}.catalogue"),
            context! { skills, load_skill => LOAD_SKILL_TOOL_NAME },
        )
    }
}

/// Why the catalogue of skills cannot be given (issue 336): what the store
/// reported, judged as store errors are; the rest in the words it has,
/// which are written to be read.
impl From<SkillError> for ContextError {
    fn from(error: SkillError) -> Self {
        match error {
            SkillError::Store(e) => ContextError::Store(e),
            error => ContextError::Failed(error.to_string()),
        }
    }
}

#[async_trait]
impl PromptProvider for SkillsComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let Some(skills) = reader_in(situation).await else {
            return Ok(Prompt::classified(String::new(), Classification::PUBLIC));
        };
        let skills = skills.usable_skills().await?;
        let classification = joined(&skills, &situation.classification());
        Ok(Prompt::classified(self.catalogue(&skills)?, classification))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Error;
    use caelumora_agents::error::user_message;
    use caelumora_domain::store::StoreError;

    #[test]
    fn what_the_catalogue_meets_reaches_the_owner_as_the_kernel_judges_it() {
        let shown = |error: SkillError| user_message(&Error::new(ContextError::from(error)));
        assert_eq!(shown(SkillError::NotJoined).as_deref(), Some("skills are not offered here"));
        assert_eq!(
            shown(SkillError::Store(StoreError::Backend(
                "/var/lib/skills: permission denied".to_string()
            ))),
            None
        );
    }
}
