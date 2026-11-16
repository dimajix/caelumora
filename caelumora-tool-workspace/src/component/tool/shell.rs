//! The shell tools of `workspace`, each a type of its own (issue 584): commands in the workspace,
//! through the `Shell` the workspace's writer hands out (`WorkspaceWriter::shell`, reached by
//! `shell`), which sees the commands started in that channel alone.
//!
//! A call dropped while it waits for its command - the turn cancelled or stopped (issue 161) -
//! kills the command (`kill_on_drop`): the one thing a tool does after its future is gone. One
//! handed back as running is the model's to end, or a user's (issue 294): `kill_command` ends it
//! through the same shell of the channel (`WorkspaceController::shell_in`) as the REST server does
//! for them.

use super::{next, of_workspace, shell, tool, Use, UsingTool};
use crate::component::WorkspaceParams;
use async_trait::async_trait;
use caelumora_agents::tool::{
    arguments, proposed_matcher, Tool, ToolError, ToolOutput, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_api::terminal::{kill_on_drop, CommandState, RUN_COMMAND, SHELL_PRESENTATION};
use caelumora_domain::approval::ToolMatcher;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters};
use caelumora_domain::workspace::CommandStatus;
use caelumora_domain::workspace::Workspace;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

pub(super) const COMMAND_OUTPUT: &str = "command_output";
pub(super) const KILL_COMMAND: &str = "kill_command";

/// The longest a call may follow a command - Open Terminal's own bound.
const MAX_WAIT_SECS: u64 = 300;

fn wait(params: &WorkspaceParams, asked: Option<f64>) -> Duration {
    let most = params.max_wait_secs.min(MAX_WAIT_SECS) as f64;
    Duration::from_secs_f64(asked.unwrap_or(most).clamp(0.0, most))
}

/// `state` as the model is given it: the output cut from the front to what a result may hold.
fn result(params: &WorkspaceParams, prompts: &PromptTexts, state: CommandState) -> Value {
    let running = state.status == CommandStatus::Running;
    let mut result = json!(state.keeping_last(params.max_output_bytes));
    if running {
        if let Some(next) = next(
            prompts,
            "still_running",
            context! { command_output => COMMAND_OUTPUT, kill_command => KILL_COMMAND },
        ) {
            result["next"] = json!(next);
        }
    }
    result
}

#[derive(Deserialize)]
struct Run {
    command: String,
    #[serde(default)]
    wait_secs: Option<f64>,
}

#[derive(Deserialize)]
struct Follow {
    id: String,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    wait_secs: Option<f64>,
}

/// The shell tools, configured by `params`, their texts from `prompts`, each used as a shell.
///
/// # Errors
/// `PromptError` if one of their texts cannot be had.
pub(super) fn tools(
    params: &Arc<WorkspaceParams>,
    prompts: &PromptTexts,
) -> Result<Vec<UsingTool>, PromptError> {
    let most = params.max_wait_secs.min(MAX_WAIT_SECS);
    let values = context! { most, run_command => RUN_COMMAND, command_output => COMMAND_OUTPUT, kill_command => KILL_COMMAND };
    let run_command = RunCommandTool {
        description: tool(
            prompts,
            RUN_COMMAND,
            ToolParameters::object(
                [("command", ToolParameters::shell()), ("wait_secs", ToolParameters::number())],
                ["command"],
            ),
            params.requires_approval.required(false),
            SHELL_PRESENTATION,
            &values,
        )?,
        params: Arc::clone(params),
        prompts: prompts.clone(),
    };
    let command_output = CommandOutputTool {
        description: tool(
            prompts,
            COMMAND_OUTPUT,
            ToolParameters::object(
                [
                    ("id", ToolParameters::string()),
                    ("offset", ToolParameters::number()),
                    ("wait_secs", ToolParameters::number()),
                ],
                ["id"],
            ),
            false,
            SHELL_PRESENTATION,
            &values,
        )?,
        params: Arc::clone(params),
        prompts: prompts.clone(),
    };
    let kill_command = KillCommandTool {
        description: tool(
            prompts,
            KILL_COMMAND,
            ToolParameters::object([("id", ToolParameters::string())], ["id"]),
            false,
            SHELL_PRESENTATION,
            &values,
        )?,
        params: Arc::clone(params),
        prompts: prompts.clone(),
    };
    Ok(vec![
        UsingTool { using: Use::Shell, tool: Arc::new(run_command) },
        UsingTool { using: Use::Shell, tool: Arc::new(command_output) },
        UsingTool { using: Use::Shell, tool: Arc::new(kill_command) },
    ])
}

/// The calls like one of the shell tool `tool` with `params`, as its parameters tell
/// (`proposed_matcher`): for a command, those beginning with its first words, however long a call
/// waits for it.
fn followed_alike(tool: &ToolDescription, params: &Value) -> ToolMatcher {
    let mut matcher = proposed_matcher(tool, params);
    matcher.arguments.remove("wait_secs");
    matcher
}

/// The result of a shell tool configured by `params`, worded by `prompts`: how a command stands
/// (`state`), classified as `workspace`.
fn standing(
    params: &WorkspaceParams,
    prompts: &PromptTexts,
    state: CommandState,
    workspace: Workspace,
) -> ToolOutput {
    of_workspace(result(params, prompts, state).into(), workspace)
}

/// `run_command`: starts a command in the workspace and follows it for a while; one still running
/// then is handed back with its id, and killed if the call is dropped while it waits.
#[derive(Debug)]
struct RunCommandTool {
    description: ToolDescription,
    params: Arc<WorkspaceParams>,
    prompts: PromptTexts,
}

#[async_trait]
impl Tool for RunCommandTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let (workspace, shell) = shell(turn).await?;
        let Run { command, wait_secs } = arguments(params)?;
        if command.trim().is_empty() {
            return Err(ToolError::InvalidParameter("'command' must name a command".to_string()));
        }
        let id = shell.start(&command).await?;
        let guard = kill_on_drop(Arc::clone(&shell), id.clone());
        let state = shell.output(&id, 0, wait(&self.params, wait_secs)).await;
        guard.disarm();
        Ok(standing(&self.params, &self.prompts, state?, workspace))
    }

    /// What an approver reads: the command, whole.
    fn describe(&self, params: &Value) -> ToolResult<String> {
        let Run { command, .. } = arguments(params)?;
        Ok(format!("Run in the workspace:\n{command}"))
    }

    fn matcher(&self, params: &Value) -> ToolMatcher {
        followed_alike(&self.description, params)
    }
}

/// `command_output`: follows a command handed back as running further.
#[derive(Debug)]
struct CommandOutputTool {
    description: ToolDescription,
    params: Arc<WorkspaceParams>,
    prompts: PromptTexts,
}

#[async_trait]
impl Tool for CommandOutputTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let (workspace, shell) = shell(turn).await?;
        let Follow { id, offset, wait_secs } = arguments(params)?;
        let state = shell.output(&id, offset.unwrap_or(0), wait(&self.params, wait_secs)).await?;
        Ok(standing(&self.params, &self.prompts, state, workspace))
    }

    fn describe(&self, params: &Value) -> ToolResult<String> {
        arguments::<Follow>(params).map(|follow| format!("Follow command {}", follow.id))
    }

    fn matcher(&self, params: &Value) -> ToolMatcher {
        followed_alike(&self.description, params)
    }
}

/// `kill_command`: ends a command handed back as running.
#[derive(Debug)]
struct KillCommandTool {
    description: ToolDescription,
    params: Arc<WorkspaceParams>,
    prompts: PromptTexts,
}

#[async_trait]
impl Tool for KillCommandTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let (workspace, shell) = shell(turn).await?;
        let Follow { id, offset, .. } = arguments(params)?;
        let state = shell.kill(&id, offset.unwrap_or(0)).await?;
        Ok(standing(&self.params, &self.prompts, state, workspace))
    }

    fn describe(&self, params: &Value) -> ToolResult<String> {
        arguments::<Follow>(params).map(|follow| format!("Kill command {}", follow.id))
    }

    fn matcher(&self, params: &Value) -> ToolMatcher {
        followed_alike(&self.description, params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::testing::component;
    use crate::component::WorkspaceComponent;
    use caelumora_agents::testing::{tool_named, workspace_turn};
    use caelumora_agents::tool::ToolProvider;
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::approval::ArgumentMatcher;
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use std::collections::BTreeMap;
    use tokio::time;

    fn the_component() -> WorkspaceComponent {
        component(json!({ "max_wait_secs": 10 }))
    }

    #[tokio::test]
    async fn a_command_runs_in_the_session_s_workspace_and_its_result_is_classified_as_it() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let ran = tool_named(&the_component(), RUN_COMMAND)
            .execute(
                &json!({ "command": "echo draft > notes.md; cat notes.md; ls" }),
                None,
                &mut turn,
            )
            .await
            .unwrap();
        assert_eq!(ran.value["status"], "completed");
        assert_eq!(ran.value["exit_code"], 0);
        assert_eq!(ran.value["output"], "draft\nnotes.md\n");
        assert_eq!(
            ran.classification.as_ref(),
            Some(&world.workspace.classification),
            "classified as the workspace"
        );
        assert!(world.dir().join("notes.md").is_file(), "written into the workspace");
    }

    #[tokio::test]
    async fn a_long_command_is_handed_back_followed_and_killed() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = the_component();
        let running = tool_named(&tool, RUN_COMMAND)
            .execute(
                &json!({ "command": "echo begun; sleep 30", "wait_secs": 0.3 }),
                None,
                &mut turn,
            )
            .await
            .unwrap();
        assert_eq!(running.value["status"], "running");
        assert!(running.value.get("exit_code").is_none(), "none while it runs");
        assert!(running.value["next"].as_str().unwrap().contains(COMMAND_OUTPUT));
        let id = running.value["id"].clone();
        let offset = running.value["next_offset"].clone();
        let followed = tool_named(&tool, COMMAND_OUTPUT)
            .execute(&json!({ "id": id, "offset": offset, "wait_secs": 0.1 }), None, &mut turn)
            .await
            .unwrap();
        assert_eq!(
            (followed.value["status"].as_str(), followed.value["output"].as_str()),
            (Some("running"), Some("")),
            "nothing new"
        );
        let killed = tool_named(&tool, KILL_COMMAND)
            .execute(&json!({ "id": id }), None, &mut turn)
            .await
            .unwrap();
        assert_eq!(killed.value["status"], "killed");
        let unknown = tool_named(&tool, COMMAND_OUTPUT)
            .execute(&json!({ "id": "nonesuch" }), None, &mut turn)
            .await;
        assert!(
            matches!(unknown, Err(ToolError::InvalidParameter(_))),
            "the model's to correct: {unknown:?}"
        );
    }

    #[tokio::test]
    async fn an_interrupted_call_kills_its_command() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = the_component();
        let marker = world.dir().join("still-running");
        let command = format!("sleep 1.5 && touch {}", marker.display());
        let interrupted = time::timeout(
            Duration::from_millis(300),
            tool_named(&tool, RUN_COMMAND).execute(&json!({ "command": command }), None, &mut turn),
        )
        .await;
        assert!(interrupted.is_err(), "dropped while it waited");
        time::sleep(Duration::from_millis(2500)).await;
        assert!(!marker.exists(), "the command was killed with the call");
    }

    #[test]
    fn the_approver_reads_the_command_whole_and_a_rule_covers_its_first_words() {
        let tool = the_component();
        assert_eq!(
            tool_named(&tool, RUN_COMMAND)
                .describe(&json!({ "command": "rm -rf build && make" }))
                .unwrap(),
            "Run in the workspace:\nrm -rf build && make"
        );
        assert!(tool
            .tools()
            .iter()
            .map(|t| t.description())
            .filter(|t| [RUN_COMMAND, COMMAND_OUTPUT, KILL_COMMAND].contains(&t.name.as_str()))
            .all(|t| t.presentation == SHELL_PRESENTATION));

        let matcher = tool_named(&tool, RUN_COMMAND)
            .matcher(&json!({ "command": "cargo test -p x", "wait_secs": 30 }));
        let first_words =
            ArgumentMatcher::Shell { shell: vec!["cargo".to_string(), "test".to_string()] };
        assert_eq!(
            matcher.arguments,
            BTreeMap::from([("command".to_string(), first_words)]),
            "however long it waits"
        );
        let chained = tool_named(&tool, RUN_COMMAND).matcher(&json!({ "command": "ls && rm x" }));
        assert_eq!(
            chained.arguments["command"],
            ArgumentMatcher::Pattern(regex::escape("ls && rm x")),
            "a chain, alone"
        );
    }
}
