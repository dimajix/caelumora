//! The `terminal_tool` component (issue 251, part C): a shell in the
//! session's workspace, for the agents of the channels that may use it.
//! Generic over the terminal: what it can do is what the workspace's
//! storage gives (`Terminal::shell`), and the component knows no provider.
//!
//! - `run_command` starts a command in the workspace's directory and follows
//!   it for a while; one still running then is handed back with its id.
//! - `command_output` follows such a command further, `kill_command` ends it.
//!
//! The workspace is reached through the turn's views on every call, before
//! anything is sent: as a member of the channel, where the channel uses it
//! and it and the channel flow both ways. A result is classified as the
//! workspace. A command is seen only from the channel it was started in.
//!
//! A call dropped while it waits for its command - the turn cancelled or
//! stopped (issue 161) - kills the command: the one thing a tool does after
//! its future is gone, in a task of its own bounded by `KILL_WITHIN`, which
//! the drop does not await. A command left running would go on writing into
//! the workspace after the turn that started it. One handed back as running
//! is the model's to end.

use crate::agent_turn::Turn;
use crate::component::{ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::terminal::{CommandState, CommandStatus, Shell, TerminalError};
use crate::tool::{arguments, ToolError, ToolOutput, ToolProvider, ToolResult};
use crate::view::WorkspaceView;
use async_trait::async_trait;
use caelumora_model::question::Answers;
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

pub const TERMINAL_TOOL_KIND: &str = "terminal_tool";

pub const RUN_COMMAND: &str = "run_command";
pub const COMMAND_OUTPUT: &str = "command_output";
pub const KILL_COMMAND: &str = "kill_command";

/// The longest a call may follow a command - Open Terminal's own bound.
const MAX_WAIT_SECS: u64 = 300;
/// How long a dropped call's kill may take.
const KILL_WITHIN: Duration = Duration::from_secs(5);

fn default_wait_secs() -> u64 {
    60
}

fn default_output_bytes() -> usize {
    64 * 1024
}

fn by_default() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalToolConfig {
    /// How long a call follows a command at most, in seconds: 60 unless
    /// configured, never more than 300.
    #[serde(default = "default_wait_secs")]
    max_wait_secs: u64,
    /// How much of a command's output one result holds at most; the rest
    /// is cut from the front. 64 KiB unless configured.
    #[serde(default = "default_output_bytes")]
    max_output_bytes: usize,
    /// Whether `run_command` needs approval - unless configured, it does: a
    /// command may act anywhere its place reaches (issue 150).
    /// `command_output` and `kill_command` concern a command that ran
    /// already, and never do.
    #[serde(default = "by_default")]
    requires_approval: bool,
    /// What the tool set is for, written for a model (issue 194); a text of
    /// its own unless configured.
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug)]
pub struct TerminalTool {
    config: TerminalToolConfig,
}

/// Kills a command if dropped before `disarm`.
struct KillOnDrop {
    shell: Arc<dyn Shell>,
    scope: String,
    id: String,
    armed: bool,
}

impl KillOnDrop {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let (shell, scope, id) = (Arc::clone(&self.shell), std::mem::take(&mut self.scope), std::mem::take(&mut self.id));
        runtime.spawn(async move {
            match tokio::time::timeout(KILL_WITHIN, shell.kill(&scope, &id, 0)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => warn!(command = %id, "The command of an interrupted call could not be killed: {e}"),
                Err(_) => warn!(command = %id, "The command of an interrupted call was not killed in time"),
            }
        });
    }
}

impl TerminalTool {
    fn wait(&self, asked: Option<f64>) -> Duration {
        let most = self.config.max_wait_secs.min(MAX_WAIT_SECS) as f64;
        Duration::from_secs_f64(asked.unwrap_or(most).clamp(0.0, most))
    }

    /// The workspace of the turn's channel, if the agent may use it there.
    async fn workspace(&self, turn: &Turn<'_>) -> ToolResult<WorkspaceView> {
        let session = turn.session().ok_or_else(|| ToolError::ExecutionFailed("the turn is in no session".to_string()))?;
        let channel = session.channel(turn.channel_id()).await?;
        channel.workspace().await?
            .ok_or_else(|| ToolError::ExecutionFailed("there is no workspace you can use in this channel".to_string()))
    }

    /// `state` as the model is given it: the output cut from the front to
    /// what a result may hold.
    fn result(&self, state: CommandState) -> Value {
        let mut output = state.output;
        let mut truncated = state.truncated;
        if output.len() > self.config.max_output_bytes {
            let mut cut = output.len() - self.config.max_output_bytes;
            while !output.is_char_boundary(cut) {
                cut += 1;
            }
            output.drain(..cut);
            truncated = true;
        }
        let mut result = json!({
            "id": state.id,
            "status": state.status,
            "output": output,
            "next_offset": state.next_offset,
            "truncated": truncated,
        });
        if let Some(code) = state.exit_code {
            result["exit_code"] = json!(code);
        }
        if state.status == CommandStatus::Running {
            result["next"] = json!(format!("It is still running: follow it with {COMMAND_OUTPUT} (id, offset = next_offset), or end it with {KILL_COMMAND}."));
        }
        result
    }
}

/// What a terminal reports, as the model is told it: a command it does not
/// know is the model's to correct.
fn failed(error: TerminalError) -> ToolError {
    match error {
        TerminalError::UnknownCommand(_) => ToolError::InvalidParameter(error.to_string()),
        error => ToolError::ExecutionFailed(error.to_string()),
    }
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

#[async_trait]
impl ToolProvider for TerminalTool {
    fn description(&self) -> String {
        self.config.description.clone().unwrap_or_else(|| {
            "A shell in the session's workspace: run commands there - on its files, which persist across turns.".to_string()
        })
    }

    fn tools(&self) -> Vec<Tool> {
        let most = self.config.max_wait_secs.min(MAX_WAIT_SECS);
        let tool = |name: &str, description: String, parameters: ToolParameters, requires_approval: bool| Tool {
            name: name.to_string(),
            description,
            parameters,
            requires_approval,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: ToolPresentation::Shell,
        };
        vec![
            tool(
                RUN_COMMAND,
                format!(
                    "Run a shell command (bash) in the session's workspace, a directory whose files persist across \
                    turns and are shared with the others who use it. Every command starts in the workspace's top \
                    directory: a `cd` does not carry over to the next. Waits up to {most} seconds for it to end, and \
                    returns what it printed - standard output and error in the order they came - and its exit code. \
                    A command still running then is returned with its id: follow it with {COMMAND_OUTPUT}, end it with \
                    {KILL_COMMAND}."
                ),
                ToolParameters::object(
                    [
                        ("command", ToolParameters::string("The command line, as bash reads it")),
                        ("wait_secs", ToolParameters::number(format!("How long to wait for it to end, at most {most} seconds; {most} unless given"))),
                    ],
                    ["command"],
                ),
                self.config.requires_approval,
            ),
            tool(
                COMMAND_OUTPUT,
                format!("New output of a command {RUN_COMMAND} returned as still running, from the next_offset of its last result, waiting for it to end as {RUN_COMMAND} does."),
                ToolParameters::object(
                    [
                        ("id", ToolParameters::string("The id of the command")),
                        ("offset", ToolParameters::number("The next_offset of the last result about it; 0 for all it printed")),
                        ("wait_secs", ToolParameters::number(format!("How long to wait for it to end, at most {most} seconds"))),
                    ],
                    ["id"],
                ),
                false,
            ),
            tool(
                KILL_COMMAND,
                "End a command that is still running, with everything it started.".to_string(),
                ToolParameters::object([("id", ToolParameters::string("The id of the command"))], ["id"]),
                false,
            ),
        ]
    }

    /// What an approver reads: the command, whole.
    fn describe(&self, name: &str, params: &Value) -> ToolResult<String> {
        match name {
            RUN_COMMAND => {
                let Run { command, .. } = arguments(params)?;
                Ok(format!("Run in the workspace:\n{command}"))
            }
            COMMAND_OUTPUT => arguments::<Follow>(params).map(|follow| format!("Follow command {}", follow.id)),
            KILL_COMMAND => arguments::<Follow>(params).map(|follow| format!("Kill command {}", follow.id)),
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        if ![RUN_COMMAND, COMMAND_OUTPUT, KILL_COMMAND].contains(&name) {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        let workspace = self.workspace(turn).await?;
        let terminal = workspace.terminal().await.map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let shell = terminal.shell().ok_or_else(|| ToolError::ExecutionFailed("the workspace's storage gives it no shell".to_string()))?;
        let scope = turn.channel_id().to_string();

        let state = match name {
            RUN_COMMAND => {
                let Run { command, wait_secs } = arguments(params)?;
                if command.trim().is_empty() {
                    return Err(ToolError::InvalidParameter("'command' must name a command".to_string()));
                }
                let id = shell.start(&scope, &command).await.map_err(failed)?;
                let guard = KillOnDrop { shell: Arc::clone(&shell), scope: scope.clone(), id: id.clone(), armed: true };
                let state = shell.output(&scope, &id, 0, self.wait(wait_secs)).await;
                guard.disarm();
                state
            }
            COMMAND_OUTPUT => {
                let Follow { id, offset, wait_secs } = arguments(params)?;
                shell.output(&scope, &id, offset.unwrap_or(0), self.wait(wait_secs)).await
            }
            _ => {
                let Follow { id, offset, .. } = arguments(params)?;
                shell.kill(&scope, &id, offset.unwrap_or(0)).await
            }
        }.map_err(failed)?;
        Ok(ToolOutput::classified(self.result(state), workspace.classification().clone()))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(TERMINAL_TOOL_KIND).provides::<dyn ToolProvider>(),
        |descriptor, _context| {
            let config: TerminalToolConfig = serde_json::from_value(descriptor.params.clone())
                .map_err(|e| ComponentError::InvalidConfig(format!("invalid parameters: {e}")))?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(TerminalTool { config }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::testing::session_with_workspace;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::security::SecurityContext;

    fn tool() -> TerminalTool {
        TerminalTool { config: serde_json::from_value(json!({ "max_wait_secs": 10 })).unwrap() }
    }

    #[tokio::test]
    async fn a_command_runs_in_the_session_s_workspace_and_its_result_is_classified_as_it() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        let ran = tool().execute(RUN_COMMAND, &json!({ "command": "echo draft > notes.md; cat notes.md; ls" }), None, &mut turn).await.unwrap();
        assert_eq!(ran.value["status"], "completed");
        assert_eq!(ran.value["exit_code"], 0);
        assert_eq!(ran.value["output"], "draft\nnotes.md\n");
        assert_eq!(ran.classification.as_ref(), Some(&world.workspace.classification), "classified as the workspace");
        assert!(world.dir().join("notes.md").is_file(), "written into the workspace");
    }

    #[tokio::test]
    async fn a_long_command_is_handed_back_followed_and_killed() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        let tool = tool();
        let running = tool.execute(RUN_COMMAND, &json!({ "command": "echo begun; sleep 30", "wait_secs": 0.3 }), None, &mut turn).await.unwrap();
        assert_eq!(running.value["status"], "running");
        assert!(running.value["next"].as_str().unwrap().contains(COMMAND_OUTPUT));
        let id = running.value["id"].clone();
        let offset = running.value["next_offset"].clone();
        let followed = tool.execute(COMMAND_OUTPUT, &json!({ "id": id, "offset": offset, "wait_secs": 0.1 }), None, &mut turn).await.unwrap();
        assert_eq!((followed.value["status"].as_str(), followed.value["output"].as_str()), (Some("running"), Some("")), "nothing new");
        let killed = tool.execute(KILL_COMMAND, &json!({ "id": id }), None, &mut turn).await.unwrap();
        assert_eq!(killed.value["status"], "killed");
        let unknown = tool.execute(COMMAND_OUTPUT, &json!({ "id": "nonesuch" }), None, &mut turn).await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))), "the model's to correct: {unknown:?}");
    }

    #[tokio::test]
    async fn an_interrupted_call_kills_its_command() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        let tool = tool();
        let marker = world.dir().join("still-running");
        let command = format!("sleep 1.5 && touch {}", marker.display());
        let interrupted = tokio::time::timeout(Duration::from_millis(300), tool.execute(RUN_COMMAND, &json!({ "command": command }), None, &mut turn)).await;
        assert!(interrupted.is_err(), "dropped while it waited");
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(!marker.exists(), "the command was killed with the call");
    }

    #[tokio::test]
    async fn no_workspace_is_used_where_the_channel_does_not_use_it() {
        let world = session_with_workspace().await;
        world.session.channels()[0].update_uses_workspace(&SecurityContext::system(), false).await.unwrap();
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        let refused = tool().execute(RUN_COMMAND, &json!({ "command": "touch here" }), None, &mut turn).await;
        assert!(matches!(&refused, Err(ToolError::ExecutionFailed(why)) if why.contains("no workspace")), "{refused:?}");
        assert!(!world.dir().join("here").exists());
    }

    #[test]
    fn the_approver_reads_the_command_whole_and_only_running_one_asks() {
        let tool = tool();
        assert_eq!(tool.describe(RUN_COMMAND, &json!({ "command": "rm -rf build && make" })).unwrap(), "Run in the workspace:\nrm -rf build && make");
        let asks: Vec<(String, bool)> = tool.tools().into_iter().map(|t| (t.name, t.requires_approval)).collect();
        assert_eq!(asks, [(RUN_COMMAND.to_string(), true), (COMMAND_OUTPUT.to_string(), false), (KILL_COMMAND.to_string(), false)]);
        assert!(tool.tools().iter().all(|t| t.presentation == ToolPresentation::Shell));
    }

    #[test]
    fn output_beyond_what_a_result_holds_is_cut_from_the_front() {
        let tool = TerminalTool { config: serde_json::from_value(json!({ "max_output_bytes": 2 })).unwrap() };
        let state = CommandState { id: "x".to_string(), status: CommandStatus::Completed, exit_code: Some(0), output: "abcdéf".to_string(), next_offset: 7, truncated: false };
        let result = tool.result(state);
        assert_eq!((result["output"].as_str(), result["truncated"].as_bool()), (Some("f"), Some(true)), "the cut falls inside é and moves past it: {result}");
    }
}
