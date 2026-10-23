//! The shell tools of `terminal_tool`: commands in the workspace, through
//! the `Shell` the turn's channel hands out (`Turn::workspace_shell`),
//! which sees the commands started in that channel alone.
//!
//! A call dropped while it waits for its command - the turn cancelled or
//! stopped (issue 161) - kills the command: the one thing a tool does after
//! its future is gone, in a task of its own bounded by `KILL_WITHIN`, which
//! the drop does not await. A command left running would go on writing into
//! the workspace after the turn that started it. One handed back as running
//! is the model's to end, or a user's (issue 294): `kill_command` ends it
//! through the same shell of the channel (`ChannelLoop::workspace_shell`)
//! as the REST server does for them.

use crate::terminal_tool::{next, tool, TerminalTool, TerminalToolConfig};
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_runtime::terminal::{CommandState, Shell};
use caelumora_model::workspace::CommandStatus;
use caelumora_agents::tool::{arguments, ToolError, ToolResult};
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;
use tokio::runtime::Handle;
use std::mem;
use tokio::time;

/// A shell command (issue 251): its `command` as a line of code, its
/// `output` as a terminal shows it, and its exit code.
const SHELL_PRESENTATION: ToolPresentation = ToolPresentation::new("shell");

pub const RUN_COMMAND: &str = "run_command";
pub const COMMAND_OUTPUT: &str = "command_output";
pub const KILL_COMMAND: &str = "kill_command";

/// The shell tools, by name.
pub(crate) const TOOLS: [&str; 3] = [RUN_COMMAND, COMMAND_OUTPUT, KILL_COMMAND];

/// The longest a call may follow a command - Open Terminal's own bound.
const MAX_WAIT_SECS: u64 = 300;
/// How long a dropped call's kill may take.
const KILL_WITHIN: Duration = Duration::from_secs(5);

/// Kills a command if dropped before `disarm`.
struct KillOnDrop {
    shell: Arc<dyn Shell>,
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
        let Ok(runtime) = Handle::try_current() else { return };
        let (shell, id) = (Arc::clone(&self.shell), mem::take(&mut self.id));
        runtime.spawn(async move {
            match time::timeout(KILL_WITHIN, shell.kill(&id, 0)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => warn!(command = %id, "The command of an interrupted call could not be killed: {e}"),
                Err(_) => warn!(command = %id, "The command of an interrupted call was not killed in time"),
            }
        });
    }
}

fn wait(config: &TerminalToolConfig, asked: Option<f64>) -> Duration {
    let most = config.max_wait_secs.min(MAX_WAIT_SECS) as f64;
    Duration::from_secs_f64(asked.unwrap_or(most).clamp(0.0, most))
}

/// `state` as the model is given it: the output cut from the front to
/// what a result may hold.
fn result(config: &TerminalToolConfig, prompts: &PromptTexts, state: CommandState) -> Value {
    let mut output = state.output;
    let mut truncated = state.truncated;
    if output.len() > config.max_output_bytes {
        let mut cut = output.len() - config.max_output_bytes;
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
        if let Some(next) = next(prompts, "still_running", context! { command_output => COMMAND_OUTPUT, kill_command => KILL_COMMAND }) {
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

pub(crate) fn tools(config: &TerminalToolConfig, prompts: &PromptTexts) -> Result<Vec<Tool>, PromptError> {
    let most = config.max_wait_secs.min(MAX_WAIT_SECS);
    let values = context! { most, run_command => RUN_COMMAND, command_output => COMMAND_OUTPUT, kill_command => KILL_COMMAND };
    Ok(vec![
        tool(
            prompts,
            RUN_COMMAND,
            ToolParameters::object([("command", ToolParameters::shell()), ("wait_secs", ToolParameters::number())], ["command"]),
            config.requires_approval.required(false),
            SHELL_PRESENTATION,
            &values,
        )?,
        tool(
            prompts,
            COMMAND_OUTPUT,
            ToolParameters::object([("id", ToolParameters::string()), ("offset", ToolParameters::number()), ("wait_secs", ToolParameters::number())], ["id"]),
            false,
            SHELL_PRESENTATION,
            &values,
        )?,
        tool(prompts, KILL_COMMAND, ToolParameters::object([("id", ToolParameters::string())], ["id"]), false, SHELL_PRESENTATION, &values)?,
    ])
}

/// What an approver reads: the command, whole.
pub(crate) fn describe(name: &str, params: &Value) -> ToolResult<String> {
    match name {
        RUN_COMMAND => {
            let Run { command, .. } = arguments(params)?;
            Ok(format!("Run in the workspace:\n{command}"))
        }
        COMMAND_OUTPUT => arguments::<Follow>(params).map(|follow| format!("Follow command {}", follow.id)),
        _ => arguments::<Follow>(params).map(|follow| format!("Kill command {}", follow.id)),
    }
}

/// Runs the shell tool `name` of `tool` with `params` on `shell`, the workspace's as
/// the turn's channel sees it.
pub(crate) async fn execute(tool: &TerminalTool, name: &str, params: &Value, shell: Arc<dyn Shell>) -> ToolResult<Value> {
    let (config, prompts) = (&tool.config, &tool.prompts);
    let state = match name {
        RUN_COMMAND => {
            let Run { command, wait_secs } = arguments(params)?;
            if command.trim().is_empty() {
                return Err(ToolError::InvalidParameter("'command' must name a command".to_string()));
            }
            let id = shell.start(&command).await?;
            let guard = KillOnDrop { shell: Arc::clone(&shell), id: id.clone(), armed: true };
            let state = shell.output(&id, 0, wait(config, wait_secs)).await;
            guard.disarm();
            state
        }
        COMMAND_OUTPUT => {
            let Follow { id, offset, wait_secs } = arguments(params)?;
            shell.output(&id, offset.unwrap_or(0), wait(config, wait_secs)).await
        }
        _ => {
            let Follow { id, offset, .. } = arguments(params)?;
            shell.kill(&id, offset.unwrap_or(0)).await
        }
    }?;
    Ok(result(config, prompts, state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::prompt_catalogue::PromptCatalogue;
    use caelumora_agents::testing::workspace_turn;
    use caelumora_agents::tool::ToolProvider;
    use crate::terminal_tool::tests::tool as terminal_tool;
    use crate::terminal_tool::TerminalTool;
    use caelumora_runtime::workspace::testing::{session_with_workspace, WithWorkspace};
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::approval::ArgumentMatcher;
    use std::collections::BTreeMap;

    fn the_tool(world: &WithWorkspace) -> TerminalTool {
        terminal_tool(world, json!({ "max_wait_secs": 10 }))
    }

    #[tokio::test]
    async fn a_command_runs_in_the_session_s_workspace_and_its_result_is_classified_as_it() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let ran = the_tool(&world).execute(RUN_COMMAND, &json!({ "command": "echo draft > notes.md; cat notes.md; ls" }), None, &mut turn).await.unwrap();
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
        let mut turn = workspace_turn(&world, &mut state);
        let tool = the_tool(&world);
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
        let mut turn = workspace_turn(&world, &mut state);
        let tool = the_tool(&world);
        let marker = world.dir().join("still-running");
        let command = format!("sleep 1.5 && touch {}", marker.display());
        let interrupted = time::timeout(Duration::from_millis(300), tool.execute(RUN_COMMAND, &json!({ "command": command }), None, &mut turn)).await;
        assert!(interrupted.is_err(), "dropped while it waited");
        time::sleep(Duration::from_millis(2500)).await;
        assert!(!marker.exists(), "the command was killed with the call");
    }

    #[tokio::test]
    async fn the_approver_reads_the_command_whole_and_a_rule_covers_its_first_words() {
        let world = session_with_workspace().await;
        let tool = the_tool(&world);
        assert_eq!(tool.describe(RUN_COMMAND, &json!({ "command": "rm -rf build && make" })).unwrap(), "Run in the workspace:\nrm -rf build && make");
        assert!(tool.tools().iter().filter(|t| TOOLS.contains(&t.name.as_str())).all(|t| t.presentation == SHELL_PRESENTATION));

        let matcher = tool.matcher(RUN_COMMAND, &json!({ "command": "cargo test -p x", "wait_secs": 30 }));
        let first_words = ArgumentMatcher::Shell { shell: vec!["cargo".to_string(), "test".to_string()] };
        assert_eq!(matcher.arguments, BTreeMap::from([("command".to_string(), first_words)]), "however long it waits");
        let chained = tool.matcher(RUN_COMMAND, &json!({ "command": "ls && rm x" }));
        assert_eq!(chained.arguments["command"], ArgumentMatcher::Pattern(regex::escape("ls && rm x")), "a chain, alone");
    }

    #[test]
    fn output_beyond_what_a_result_holds_is_cut_from_the_front() {
        let config: TerminalToolConfig = serde_json::from_value(json!({ "max_output_bytes": 2 })).unwrap();
        let state = CommandState { id: "x".to_string(), status: CommandStatus::Completed, exit_code: Some(0), output: "abcdéf".to_string(), next_offset: 7, truncated: false };
        // No text of how to go on is needed for a command that ended.
        let result = result(&config, &PromptTexts::new(Arc::new(PromptCatalogue::new()), None), state);
        assert_eq!((result["output"].as_str(), result["truncated"].as_bool()), (Some("f"), Some(true)), "the cut falls inside é and moves past it: {result}");
    }
}
