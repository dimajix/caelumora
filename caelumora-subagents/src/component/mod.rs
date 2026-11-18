//! The component kind `subagents` (issue 558): what it is configured with - the personalities its
//! tools may start an agent of, and whether starting one asks for approval -; its tools
//! `start_subagent`, `ask_subagent` and `end_subagent` are `tool`'s; the component is their
//! `ToolProvider`, which offers them where the work can start a sub-agent.

mod tool;

pub use tool::{ASK_SUBAGENT, END_SUBAGENT, START_SUBAGENT};

use crate::extension::SubAgentWriter;
use crate::NAMESPACE;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::tool::{tool_text, ApprovalMode, Tool, ToolProvider};
use caelumora_api::component::{
    ComponentError, ComponentFacets, ComponentKind, ComponentResult, LibraryComponents, Params,
};
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::value::name::Name;
use serde::Deserialize;
use std::sync::Arc;

/// The kind of the component, and the path of its texts in the catalogue.
pub const SUBAGENTS_KIND: &str = "subagents";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubagentsParams {
    /// The names of the personalities its tools may start an agent of, one at least: whose
    /// components bring the sub-agent's tools and everything else.
    personalities: Vec<Name>,
    /// Whether starting a sub-agent needs approval: unless configured, it does not - the
    /// personalities listed bound what may be started.
    #[serde(default = "ApprovalMode::never")]
    requires_approval: ApprovalMode,
}

impl Params for SubagentsParams {
    fn check(&self) -> ComponentResult<()> {
        if self.personalities.is_empty() {
            let wanted = "personalities: a personality to start sub-agents of, at least";
            return Err(ComponentError::InvalidConfig(wanted.to_string()));
        }
        Ok(())
    }
}

/// A component of the kind `subagents`: its three tools, offered where the work can start a
/// sub-agent.
#[derive(Debug)]
struct SubagentsComponent {
    /// What its tool set is for, as the prompt catalogue words it (issue 57).
    description: String,
    tools: Vec<Arc<dyn Tool>>,
}

impl SubagentsComponent {
    /// The component configured by `params`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    fn new(params: SubagentsParams, prompts: &PromptTexts) -> Result<Self, PromptError> {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(tool::StartSubagentTool::new(
                prompts,
                params.personalities,
                params.requires_approval.required(false),
            )?),
            Arc::new(tool::AskSubagentTool::new(prompts)?),
            Arc::new(tool::EndSubagentTool::new(prompts)?),
        ];
        let description = tool_text(prompts, NAMESPACE, SUBAGENTS_KIND, "description", ())?;
        Ok(Self { description, tools })
    }
}

/// Registers the component kind `subagents`, which starts, asks and ends sub-agents through the
/// plugin's writer, and writes for the agent's model.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(SUBAGENTS_KIND)
                .params::<SubagentsParams>()
                .provides::<dyn ToolProvider>()
                .writes::<dyn SubAgentWriter>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let params = SubagentsParams::read(&descriptor.params)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(SubagentsComponent::new(params, &texts)?));
            Ok(facets)
        },
    );
}
