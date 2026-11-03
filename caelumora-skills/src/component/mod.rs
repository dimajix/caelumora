//! The component kind `skills` (issues 22, 534): what an agent can look up - skills, procedures
//! written down for it to follow - and how a skill reaches it; configured with where its catalogue
//! goes and whether writing a skill is asked. It serves four facets: the catalogue, a prompt part
//! (`prompt`); the tools that list, load, write and change skills (`tool`); the commands with which
//! a participant gives the agent a skill (`command`); and the skills given again after a compaction
//! (`knowledge`).
//!
//! A skill never reaches the model through the system prompt or as a
//! message of the channel: only through the agent's own transcript, so that
//! the prompt cache holds, what it saw is recorded by digest, and nobody in
//! the channel is handed a text they may not read. Every load is recorded
//! as knowledge (`KnowledgeSource::Entity`), and a skill loaded in what a
//! compaction summarises is given again, as it is now - where it may still
//! be loaded (`KnowledgeRestorer`, issue 323).

mod command;
mod knowledge;
mod prompt;
mod tool;

pub use command::SKILL_COMMAND_NAME;
pub use tool::{
    CREATE_SKILL_TOOL_NAME, LIST_SKILLS_TOOL_NAME, LOAD_SKILL_TOOL_NAME, SKILL_LIST_PRESENTATION,
    SKILL_PRESENTATION, UPDATE_SKILL_TOOL_NAME,
};

use crate::extension::{SkillReader, SkillWriter};
use crate::skill::Skill;
use crate::NAMESPACE;
use caelumora_agents::command::AgentCommandProvider;
use caelumora_agents::prompt::{texts_of, writing_for_the_model, PromptPosition, PromptProvider};
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{tool_text, ApprovalMode, ToolProvider};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents, Params};
use caelumora_compaction::KnowledgeRestorer;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::tool::Tool;
use serde::Deserialize;
use std::sync::Arc;

/// The kind of the component, and the path of its texts in the catalogue.
pub const SKILLS_KIND: &str = "skills";

/// What the component is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillsParams {
    #[serde(default)]
    position: PromptPosition,
    #[serde(default)]
    requires_approval: ApprovalMode,
}

impl Params for SkillsParams {}

/// A component of the kind `skills`: the catalogue of the skills its agent may load, the tools and
/// commands on them, and the skills given again after a compaction.
#[derive(Debug)]
struct SkillsComponent {
    position: PromptPosition,
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them (issue 57).
    description: String,
    tools: Vec<Tool>,
    /// The texts of its catalogue and of a skill given.
    prompts: PromptTexts,
}

impl SkillsComponent {
    /// The component configured by `params`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of them cannot be had.
    fn new(params: SkillsParams, prompts: PromptTexts) -> Result<Self, PromptError> {
        let SkillsParams { position, requires_approval } = params;
        let tools = tool::tools(&prompts, requires_approval)?;
        let description = tool_text(&prompts, NAMESPACE, SKILLS_KIND, "description", ())?;
        Ok(Self { position, description, tools, prompts })
    }

    /// The text of `skill` as the model is given it, titled with its name:
    /// the one rendering of every way it reaches a model - loaded by the
    /// model, given with `/skill`, given again after a compaction (issues
    /// 16, 22).
    fn given(&self, skill: &Skill) -> Result<String, PromptError> {
        self.prompts.render(
            &format!("{NAMESPACE}.skill"),
            context! { name => skill.name.name.to_string(), body => skill.body.as_str() },
        )
    }
}

/// The classification of what `skills` hold together, within `bound` -
/// the turn's: their join where it surely flows there, else the bound.
fn joined(skills: &[Skill], bound: &Classification) -> Classification {
    skills.iter().fold(Classification::PUBLIC, |joined, skill| {
        joined.join_within(&skill.classification, bound)
    })
}

/// The skills `situation` may load, as the plugin binds their reader to the views of its work
/// (`Situation::reader`, issue 516); `None` for work that reaches no session, which has none.
async fn reader_in(situation: &Situation<'_>) -> Option<Arc<dyn SkillReader>> {
    situation.reader::<dyn SkillReader>().await
}

/// Registers the component kind `skills`, which reads and writes skills through the plugin's
/// reader and writer of them, and writes for the agent's model.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(SKILLS_KIND)
                .params::<SkillsParams>()
                .provides::<dyn PromptProvider>()
                .provides::<dyn ToolProvider>()
                .provides::<dyn AgentCommandProvider>()
                .provides::<dyn KnowledgeRestorer>()
                .reads::<dyn SkillReader>()
                .writes::<dyn SkillWriter>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let params = SkillsParams::read(&descriptor.params)?;
            let component = Arc::new(SkillsComponent::new(params, texts)?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::<SkillsComponent>::clone(&component));
            facets.insert::<dyn ToolProvider>(Arc::<SkillsComponent>::clone(&component));
            facets.insert::<dyn AgentCommandProvider>(Arc::<SkillsComponent>::clone(&component));
            facets.insert::<dyn KnowledgeRestorer>(component);
            Ok(facets)
        },
    );
}
