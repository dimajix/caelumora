//! The component kind `subagents` (issue 558): what it is configured with - the personalities its
//! tools and its command may start an agent of, and whether starting one by a tool asks for
//! approval -; its tools `start_subagent`, `ask_subagent` and `end_subagent` are `tool`'s, its
//! command `/subagent`, with which the agent's owner has it start one (issue 596), `command`'s;
//! the component is the tools' `ToolProvider`, which offers them where the work can start a
//! sub-agent.

mod command;
mod tool;

pub use command::SUBAGENT_COMMAND_NAME;
pub use tool::{ASK_SUBAGENT, END_SUBAGENT, START_SUBAGENT};

use crate::extension::SubAgentWriter;
use crate::NAMESPACE;
use caelumora_agents::command::AgentCommand;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::tool::{tool_text, ApprovalMode, Tool, ToolProvider};
use caelumora_api::command::CommandProvider;
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
    /// The names of the personalities its tools and its command may start an agent of, one at
    /// least: whose components bring the sub-agent's tools and everything else.
    personalities: Vec<Name>,
    /// Whether starting a sub-agent by its tool needs approval: unless configured, it does not -
    /// the personalities listed bound what may be started. The command needs none: whoever may
    /// issue it would approve it.
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

/// `personalities` as whoever starts a sub-agent is told them, when they named none of them.
fn listed(personalities: &[Name]) -> String {
    let names: Vec<&str> = personalities.iter().map(Name::as_str).collect();
    format!("the personalities to start a sub-agent of: {}", names.join(", "))
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
    /// The component with tools starting agents of `personalities`, asked for as
    /// `requires_approval` says, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    fn new(
        personalities: Vec<Name>,
        requires_approval: ApprovalMode,
        prompts: &PromptTexts,
    ) -> Result<Self, PromptError> {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(tool::StartSubagentTool::new(
                prompts,
                personalities,
                requires_approval.required(false),
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
                .provides::<dyn CommandProvider<dyn AgentCommand>>()
                .writes::<dyn SubAgentWriter>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let SubagentsParams { personalities, requires_approval } =
                SubagentsParams::read(&descriptor.params)?;
            let command = command::SubagentCommand::new(personalities.clone(), texts.clone());
            let commands: Vec<Arc<dyn AgentCommand>> = vec![Arc::new(command)];
            let tools = SubagentsComponent::new(personalities, requires_approval, &texts)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tools));
            facets.insert::<dyn CommandProvider<dyn AgentCommand>>(Arc::new(commands));
            Ok(facets)
        },
    );
}
