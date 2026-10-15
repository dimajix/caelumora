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
//! The workspace is reached through the turn's channel on every call
//! (`Turn::workspace_terminal`, issue 344), before anything is sent,
//! in its context (issue 296): as a member of the channel, where the channel uses it, its access
//! list lets the agent read it - change it, for the shell and the tools that
//! write (issue 23) - and it and the channel flow both ways. A result is
//! classified as the workspace. Which calls need approval is configured as for `mcp_tool`
//! (`requires_approval`, an `ApprovalMode`): all, none, or all but those
//! that only read; `command_output`, `kill_command` and `publish_file`
//! never ask.

mod files;
mod shell;

use crate::turn::Turn;
use caelumora_runtime::component::{config_of_params, ComponentFacets, ComponentKind, ComponentRegistry};
use caelumora_runtime::terminal::TerminalError;
use caelumora_runtime::workspace::{WorkspaceError, WorkspaceManager};
use crate::tool::{ApprovalMode, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::approval::ToolMatcher;
use caelumora_model::question::Answers;
use caelumora_model::security::Grant;
use caelumora_model::tool::{Tool, ToolCall, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const TERMINAL_TOOL_KIND: &str = "terminal_tool";

pub use files::{DELETE_FILE, GREP_FILES, LIST_FILES, MOVE_FILE, PUBLISH_FILE, READ_FILE, REPLACE_IN_FILE, WRITE_FILE};
pub use shell::{COMMAND_OUTPUT, KILL_COMMAND, RUN_COMMAND};

fn default_wait_secs() -> u64 {
    60
}

fn default_output_bytes() -> usize {
    64 * 1024
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalToolConfig {
    /// How long a call follows a command at most, in seconds: 60 unless
    /// configured, never more than 300.
    #[serde(default = "default_wait_secs")]
    max_wait_secs: u64,
    /// How much of a command's output, or of a file, one result holds at
    /// most. 64 KiB unless configured.
    #[serde(default = "default_output_bytes")]
    max_output_bytes: usize,
    /// Which calls need approval - unless configured, all that may: a
    /// command may act anywhere its place reaches (issue 150).
    /// `"unless_read_only"` spares the tools that only read files.
    /// `command_output` and `kill_command` concern a command that ran
    /// already, and never do; nor does `publish_file`, which gives nothing
    /// away (issue 275).
    #[serde(default)]
    requires_approval: ApprovalMode,
    /// What the tool set is for, written for a model (issue 194); a text of
    /// its own unless configured.
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug)]
pub struct TerminalTool {
    config: TerminalToolConfig,
    /// Where the largest file it may write is said.
    workspaces: Arc<WorkspaceManager>,
}

/// A tool of this component, as the model is offered it.
fn tool(name: &str, description: String, parameters: ToolParameters, requires_approval: bool, presentation: ToolPresentation) -> Tool {
    Tool { name: name.to_string(), description, parameters, requires_approval, visibility: ToolVisibility::SHOWN, classification: None, presentation }
}

/// What a terminal reports, as the model is told it: what names something
/// that is not there, or is not what the call needs, is the model's to
/// correct; that the storage failed is not.
impl From<TerminalError> for ToolError {
    fn from(error: TerminalError) -> Self {
        match error {
            TerminalError::Unavailable(_) => ToolError::ExecutionFailed(error.to_string()),
            TerminalError::InvalidPath(_) | TerminalError::NotFound(_) | TerminalError::Exists(_) | TerminalError::IsADirectory(_)
            | TerminalError::NotADirectory(_) | TerminalError::NotEmpty(_) | TerminalError::TooLarge { .. } | TerminalError::UnknownCommand(_) => {
                ToolError::InvalidParameter(error.to_string())
            }
        }
    }
}

/// That the workspace could not be used as a call needs it, as the model is
/// told it.
fn unusable(error: WorkspaceError) -> ToolError {
    ToolError::ExecutionFailed(error.to_string())
}

#[async_trait]
impl ToolProvider for TerminalTool {
    fn description(&self) -> String {
        self.config.description.clone().unwrap_or_else(|| {
            "The session's workspace: its files, which persist across turns and are shared with the others who use it - \
            list, read, search and change them - and a shell in it, where its storage gives one.".to_string()
        })
    }

    fn tools(&self) -> Vec<Tool> {
        let mut tools = shell::tools(&self.config);
        tools.extend(files::tools(&self.config));
        tools
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
    /// a move, that move.
    fn matcher(&self, name: &str, params: &Value) -> ToolMatcher {
        files::matcher(name, params).unwrap_or_else(|| {
            ToolMatcher::exact(&ToolCall { id: None, function_name: name.to_string(), arguments: params.clone(), visibility: ToolVisibility::SHOWN, presentation: Default::default() })
        })
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        let is_shell = shell::TOOLS.contains(&name);
        if !is_shell && !files::TOOLS.contains(&name) {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        let (workspace, value) = match is_shell {
            true => {
                let (workspace, shell) = turn.workspace_shell().await.map_err(unusable)?;
                let shell = shell.ok_or_else(|| ToolError::ExecutionFailed("the workspace's storage gives it no shell".to_string()))?;
                (workspace, shell::execute(&self.config, name, params, shell).await?)
            }
            false => {
                // What only reads needs only reading (issue 23).
                let need = if files::read_only(name) { Grant::Read } else { Grant::Write };
                let (workspace, terminal) = turn.workspace_terminal(need).await.map_err(unusable)?;
                let value = files::execute(&self.config, name, params, terminal.as_ref(), &workspace, self.workspaces.max_file_bytes(), turn).await?;
                (workspace, value)
            }
        };
        Ok(ToolOutput::classified(value, workspace.classification))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(TERMINAL_TOOL_KIND).provides::<dyn ToolProvider>().uses::<WorkspaceManager>(),
        |descriptor, context| {
            let config: TerminalToolConfig = config_of_params(&descriptor.params)?;
            let workspaces = Arc::clone(context.service::<WorkspaceManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(TerminalTool { config, workspaces }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::workspace_turn;
    use caelumora_runtime::component::{ComponentContainer, ComponentContext};
    use caelumora_runtime::workspace::testing::{session_with_workspace, WithWorkspace};
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::security::SecurityContext;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::type_map::TypeKey;
    use serde_json::json;

    pub(super) fn tool(world: &WithWorkspace, params: Value) -> TerminalTool {
        TerminalTool { config: serde_json::from_value(params).unwrap(), workspaces: Arc::clone(world.runtime.workspace_manager()) }
    }

    #[tokio::test]
    async fn no_workspace_is_used_where_the_channel_does_not_use_it() {
        let world = session_with_workspace().await;
        world.session.channels()[0].update_uses_workspace(&SecurityContext::system(), false).await.unwrap();
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = tool(&world, json!({}));
        for (name, params) in [(RUN_COMMAND, json!({ "command": "touch here" })), (WRITE_FILE, json!({ "path": "here", "content": "x" }))] {
            let refused = tool.execute(name, &params, None, &mut turn).await;
            assert!(matches!(&refused, Err(ToolError::ExecutionFailed(why)) if why.contains("no workspace")), "{name}: {refused:?}");
        }
        assert!(!world.dir().join("here").exists());
    }

    /// Written by hand without `params`, as the seed does: every parameter
    /// takes its default.
    #[tokio::test]
    async fn is_built_without_parameters() {
        let mut registry = ComponentRegistry::new(Arc::new(MemoryStore::new()));
        register(&mut registry);
        let config = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("terminal", TERMINAL_TOOL_KIND)] };
        let world = session_with_workspace().await;
        let context = ComponentContext::new(SecurityContext::default(), world.runtime.services().clone());
        let container = ComponentContainer::from_config(&config, &[TypeKey::of::<dyn ToolProvider>()], &Arc::new(registry), &context).await;
        assert!(container.is_ok(), "{:?}", container.err());
    }

    #[tokio::test]
    async fn which_calls_ask_is_configured_for_all_none_or_all_but_what_only_reads() {
        let world = session_with_workspace().await;
        let asking = |mode: Value| -> Vec<(String, bool)> { tool(&world, json!({ "requires_approval": mode })).tools().into_iter().map(|t| (t.name, t.requires_approval)).collect() };
        let asks = |mode: Value, name: &str| asking(mode).into_iter().find(|(n, _)| n == name).map(|(_, asks)| asks).unwrap();
        for name in [RUN_COMMAND, WRITE_FILE, REPLACE_IN_FILE, MOVE_FILE, DELETE_FILE, READ_FILE] {
            assert!(asks(json!(true), name), "{name}, by default too");
            assert!(!asks(json!(false), name), "{name}");
        }
        for (name, asked) in [(RUN_COMMAND, true), (WRITE_FILE, true), (REPLACE_IN_FILE, true), (MOVE_FILE, true), (DELETE_FILE, true), (LIST_FILES, false), (READ_FILE, false), (GREP_FILES, false), (PUBLISH_FILE, false)] {
            assert_eq!(asks(json!("unless_read_only"), name), asked, "{name}");
        }
        for name in [COMMAND_OUTPUT, KILL_COMMAND] {
            assert!(!asks(json!(true), name), "{name} concerns a command approved already");
        }
        assert!(!asks(json!(true), PUBLISH_FILE), "publishing gives nothing away");
        let defaults = tool(&world, json!({}));
        assert!(defaults.tools().iter().all(|t| t.requires_approval || [COMMAND_OUTPUT, KILL_COMMAND, PUBLISH_FILE].contains(&t.name.as_str())), "all that may, unless configured");
    }
}
