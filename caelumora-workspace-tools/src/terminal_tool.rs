//! The `terminal_tool` component (issue 251, part C): the session's
//! workspace, for the agents of the channels that may use it - its files,
//! and a shell in it where its storage gives one. Generic over the
//! terminal: what it can do is what the workspace's storage gives, and the
//! component knows no provider.
//!
//! - The shell (`shell`): `run_command` starts a command in the workspace's
//!   directory and follows it for a while; one still running then is handed
//!   back with its id. `command_output` follows it further, `kill_command`
//!   ends it.
//! - The files (`files`): `list_files`, `read_file`, `grep_files` and
//!   `publish_file` read; `write_file`, `replace_in_file`, `move_file` and
//!   `delete_file` change.
//!   What was read is recorded as the turn's knowledge
//!   (`KnowledgeSource::Workspace`).
//!
//! A turn is offered the tools that may be called on the workspace its
//! channel may use (issue 290): none where there is none, none that change
//! where its access list grants reading alone, no shell where its storage
//! gives none. The workspace is reached through the turn's channel on every
//! call all the same (`Turn::workspace_terminal`, issue 344), before
//! anything is sent,
//! in its context (issue 296): as a participant of the channel, where the channel uses it, its
//! access list lets the agent read it - change it, for the shell and the tools that
//! write (issue 23) - and it and the channel flow both ways. A result is
//! classified as the workspace. Which calls need approval is configured as for `mcp_tool`
//! (`requires_approval`, an `ApprovalMode`): all, none, or all but those
//! that only read; `command_output`, `kill_command` and `publish_file`
//! never ask.

pub use files::{
    DELETE_FILE, GREP_FILES, LIST_FILES, MOVE_FILE, PUBLISH_FILE, READ_FILE, REPLACE_IN_FILE,
    WRITE_FILE,
};
pub use shell::{COMMAND_OUTPUT, KILL_COMMAND, RUN_COMMAND};

use crate::{files, shell, NAMESPACE};
use async_trait::async_trait;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{
    described, proposed_matcher, tool_text, usable_workspace, ApprovalMode, ToolError, ToolOutput,
    ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_model::approval::ToolMatcher;
use caelumora_model::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_model::question::Answers;
use caelumora_model::security::Grant;
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use caelumora_runtime::component::{
    config_of_params, ComponentFacets, ComponentKind, ComponentRegistry,
};
use caelumora_runtime::workspace::WorkspaceManager;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tracing::error;

pub const TERMINAL_TOOL_KIND: &str = "terminal_tool";

fn default_wait_secs() -> u64 {
    60
}

fn default_output_bytes() -> usize {
    64 * 1024
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TerminalToolConfig {
    /// How long a call follows a command at most, in seconds: 60 unless
    /// configured, never more than 300.
    #[serde(default = "default_wait_secs")]
    pub(crate) max_wait_secs: u64,
    /// How much of a command's output, or of a file, one result holds at
    /// most. 64 KiB unless configured.
    #[serde(default = "default_output_bytes")]
    pub(crate) max_output_bytes: usize,
    /// Which calls need approval - unless configured, all that may: a
    /// command may act anywhere its place reaches (issue 150).
    /// `"unless_read_only"` spares the tools that only read files.
    /// `command_output` and `kill_command` concern a command that ran
    /// already, and never do; nor does `publish_file`, which gives nothing
    /// away (issue 275).
    #[serde(default)]
    pub(crate) requires_approval: ApprovalMode,
    /// What the tool set is for, written for a model (issue 194); a text of
    /// its own unless configured.
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Debug)]
pub struct TerminalTool {
    pub(crate) config: TerminalToolConfig,
    /// Where the largest file it may write is said.
    pub(crate) workspaces: Arc<WorkspaceManager>,
    /// What its tool set is for and its tools, as the configuration or the
    /// prompt catalogue words them (issue 57).
    description: String,
    tools: Vec<Tool>,
    /// What its results say of how to go on.
    pub(crate) prompts: PromptTexts,
}

impl TerminalTool {
    /// The tool, configured by `config`, with the largest file it may write
    /// said by `workspaces`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of them cannot be had.
    pub(crate) fn new(
        config: TerminalToolConfig,
        workspaces: Arc<WorkspaceManager>,
        prompts: PromptTexts,
    ) -> Result<Self, PromptError> {
        let mut tools = shell::tools(&config, &prompts)?;
        tools.extend(files::tools(&config, &prompts)?);
        let description = match &config.description {
            Some(description) => description.clone(),
            None => tool_text(&prompts, NAMESPACE, TERMINAL_TOOL_KIND, "description", ())?,
        };
        Ok(Self { config, workspaces, description, tools, prompts })
    }
}

/// A tool of this component, `values` told its texts (issue 57), as the
/// model is offered it.
///
/// # Errors
/// `PromptError` if one of its texts cannot be had.
pub(crate) fn tool(
    prompts: &PromptTexts,
    name: &str,
    parameters: ToolParameters,
    requires_approval: bool,
    presentation: ToolPresentation,
    values: impl Serialize,
) -> Result<Tool, PromptError> {
    let tool = Tool {
        name: name.to_string(),
        description: String::new(),
        parameters,
        requires_approval,
        visibility: ToolVisibility::SHOWN,
        classification: None,
        presentation,
    };
    described(prompts, NAMESPACE, TERMINAL_TOOL_KIND, tool, values)
}

/// What a result says of how to go on, the text `key` of the results
/// (`results.<key>`) rendered with `values`; `None`, logged, if it cannot be
/// had - the result is given without it, as what happened is given without
/// what to do next where a message is missing (issue 57).
pub(crate) fn next(prompts: &PromptTexts, key: &str, values: impl Serialize) -> Option<String> {
    prompts
        .render(&format!("{NAMESPACE}.results.{key}"), values)
        .inspect_err(|e| {
            error!("A result of the workspace's tools is given without saying how to go on: {e}")
        })
        .ok()
}

/// What a call of the tool `name` needs of the workspace, both to be offered
/// (issue 290) and to run: what its access list must grant (issue 23) -
/// reading for the tools that only read files, changing for the others -
/// and whether a shell; a command may change anything its place reaches.
/// `None` for a name that is none of its tools.
fn needs(name: &str) -> Option<(Grant, bool)> {
    match name {
        _ if shell::TOOLS.contains(&name) => Some((Grant::Write, true)),
        _ if files::read_only(name) => Some((Grant::Read, false)),
        _ if files::TOOLS.contains(&name) => Some((Grant::Write, false)),
        _ => None,
    }
}

#[async_trait]
impl ToolProvider for TerminalTool {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    /// The tools that may be called on the workspace the work may use in
    /// its channel (issue 290): none where there is none, none that change
    /// where its list grants reading alone, no shell where its storage gives
    /// none.
    async fn offered(&self, situation: &Situation<'_>) -> Result<Vec<Tool>, String> {
        let workspace = usable_workspace(situation).await?;
        let (writable, has_shell) = (workspace.may_write().await, workspace.has_shell());
        Ok(self
            .tools()
            .into_iter()
            .filter(|tool| {
                needs(&tool.name).is_some_and(|(grant, shell)| {
                    (grant == Grant::Read || writable) && (!shell || has_shell)
                })
            })
            .collect())
    }

    /// What an approver reads: the command, or the change, whole.
    fn describe(&self, name: &str, params: &Value) -> ToolResult<String> {
        match name {
            _ if shell::TOOLS.contains(&name) => shell::describe(name, params),
            _ if files::TOOLS.contains(&name) => files::describe(name, params),
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }

    /// For a change of a file, its path: a rule derived from an approved
    /// write covers the file, whatever is written into it; one derived from
    /// a move, that move. For a command, as its parameters tell
    /// (`proposed_matcher`): the commands beginning with its first words,
    /// however long a call waits for them.
    fn matcher(&self, name: &str, params: &Value) -> ToolMatcher {
        files::matcher(name, params).unwrap_or_else(|| {
            let mut matcher = proposed_matcher(&self.tools(), name, params);
            matcher.arguments.remove("wait_secs");
            matcher
        })
    }

    /// Checked again as it runs, whatever was offered: the workspace, and
    /// what its list grants, may have changed since the turn began.
    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        let Some((need, is_shell)) = needs(name) else {
            return Err(ToolError::UnknownTool(name.to_string()));
        };
        let (workspace, value) = match is_shell {
            true => {
                let (workspace, shell) = turn.workspace_shell().await?;
                let shell = shell.ok_or_else(|| {
                    ToolError::ExecutionFailed(
                        "the workspace's storage gives it no shell".to_string(),
                    )
                })?;
                (workspace, shell::execute(self, name, params, shell).await?)
            }
            false => {
                let (workspace, terminal) = turn.workspace_terminal(need).await?;
                let value =
                    files::execute(self, name, params, terminal.as_ref(), &workspace, turn).await?;
                (workspace, value)
            }
        };
        Ok(ToolOutput::classified(value, workspace.classification))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(TERMINAL_TOOL_KIND)
                .provides::<dyn ToolProvider>()
                .uses::<WorkspaceManager>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let config: TerminalToolConfig = config_of_params(&descriptor.params)?;
            let workspaces = Arc::clone(context.service::<WorkspaceManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(TerminalTool::new(
                config, workspaces, prompts,
            )?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{catalogue, prompts};
    use caelumora_agents::testing::with_agent_kinds;
    use caelumora_agents::testing::workspace_turn;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::security::{Policies, SecurityContext};
    use caelumora_model::store::{EntityStores, Persistence};
    use caelumora_model::type_map::TypeKey;
    use caelumora_runtime::component::{ComponentContainer, ComponentContext};
    use caelumora_runtime::workspace::testing::{session_with_workspace, WithWorkspace};
    use serde_json::json;

    pub(crate) fn tool(world: &WithWorkspace, params: Value) -> TerminalTool {
        TerminalTool::new(
            serde_json::from_value(params).unwrap(),
            Arc::clone(world.runtime.workspace_manager()),
            prompts(),
        )
        .unwrap()
    }

    /// Issue 290: offered where the channel uses the workspace - every tool,
    /// where the agent may change its files and its storage gives a shell -
    /// and not where it does not; a call is refused there all the same.
    #[tokio::test]
    async fn no_workspace_is_offered_nor_used_where_the_channel_does_not_use_it() {
        let world = session_with_workspace().await;
        let tool = tool(&world, json!({}));
        let mut state = AgentChannelState::default();
        let offered = tool
            .offered(&workspace_turn(&world, &mut state).situation())
            .await
            .expect("offered where it is used");
        assert_eq!(
            offered.len(),
            shell::TOOLS.len() + files::TOOLS.len(),
            "every tool: {offered:?}"
        );

        world.session.channels()[0]
            .update_uses_workspace(&SecurityContext::system(), false)
            .await
            .unwrap();
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let unavailable = tool.offered(&turn.situation()).await;
        assert_eq!(
            unavailable,
            Err("there is no workspace you can use in this channel".to_string()),
            "none, saying why"
        );
        for (name, params) in [
            (RUN_COMMAND, json!({ "command": "touch here" })),
            (WRITE_FILE, json!({ "path": "here", "content": "x" })),
        ] {
            let refused = tool.execute(name, &params, None, &mut turn).await;
            assert!(
                matches!(&refused, Err(ToolError::ExecutionFailed(why)) if why.contains("no workspace")),
                "{name}: {refused:?}"
            );
        }
        assert!(!world.dir().join("here").exists());
    }

    /// Written by hand without `params`, as the seed does: every parameter
    /// takes its default.
    #[tokio::test]
    async fn is_built_without_parameters() {
        let mut registry = ComponentRegistry::new(
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
        );
        // The agents' kinds, with a model whose profile names the variant.
        with_agent_kinds(&mut registry);
        register(&mut registry);
        let config = ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("terminal", TERMINAL_TOOL_KIND)],
        };
        let world = session_with_workspace().await;
        // The world runs the core alone: the texts of these tools are added.
        let context = ComponentContext::new(
            SecurityContext::default(),
            world.runtime.services().clone().with(catalogue()),
        );
        let container = ComponentContainer::from_config(
            &config,
            &[TypeKey::of::<dyn ToolProvider>()],
            &Arc::new(registry),
            &context,
        )
        .await;
        assert!(container.is_ok(), "{:?}", container.err());
    }

    #[tokio::test]
    async fn which_calls_ask_is_configured_for_all_none_or_all_but_what_only_reads() {
        let world = session_with_workspace().await;
        let asking = |mode: Value| -> Vec<(String, bool)> {
            tool(&world, json!({ "requires_approval": mode }))
                .tools()
                .into_iter()
                .map(|t| (t.name, t.requires_approval))
                .collect()
        };
        let asks = |mode: Value, name: &str| {
            asking(mode).into_iter().find(|(n, _)| n == name).map(|(_, asks)| asks).unwrap()
        };
        for name in [RUN_COMMAND, WRITE_FILE, REPLACE_IN_FILE, MOVE_FILE, DELETE_FILE, READ_FILE] {
            assert!(asks(json!(true), name), "{name}, by default too");
            assert!(!asks(json!(false), name), "{name}");
        }
        for (name, asked) in [
            (RUN_COMMAND, true),
            (WRITE_FILE, true),
            (REPLACE_IN_FILE, true),
            (MOVE_FILE, true),
            (DELETE_FILE, true),
            (LIST_FILES, false),
            (READ_FILE, false),
            (GREP_FILES, false),
            (PUBLISH_FILE, false),
        ] {
            assert_eq!(asks(json!("unless_read_only"), name), asked, "{name}");
        }
        for name in [COMMAND_OUTPUT, KILL_COMMAND] {
            assert!(!asks(json!(true), name), "{name} concerns a command approved already");
        }
        assert!(!asks(json!(true), PUBLISH_FILE), "publishing gives nothing away");
        let defaults = tool(&world, json!({}));
        assert!(
            defaults.tools().iter().all(|t| t.requires_approval
                || [COMMAND_OUTPUT, KILL_COMMAND, PUBLISH_FILE].contains(&t.name.as_str())),
            "all that may, unless configured"
        );
    }
}
