//! The shell tools of `workspace`, each a type of its own (issue 584): commands in the workspace,
//! through the `Shell` the workspace's writer hands out (`WorkspaceWriter::shell`, reached by
//! `shell`), which sees the commands started in that channel alone.
//!
//! A call dropped while it waits for its command - the turn cancelled or
//! stopped (issue 161) - kills the command: the one thing a tool does after
//! its future is gone, in a task of its own bounded by `KILL_WITHIN`, which
//! the drop does not await. A command left running would go on writing into
//! the workspace after the turn that started it. One handed back as running
//! is the model's to end, or a user's (issue 294): `kill_command` ends it
//! through the same shell of the channel (`WorkspaceController::shell_in`)
//! as the REST server does for them.

use super::{next, of_workspace, shell, tool, Use, UsingTool};
use crate::component::WorkspaceParams;
use async_trait::async_trait;
use caelumora_agents::tool::{arguments, Tool, ToolError, ToolOutput, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_api::terminal::Shell;
use caelumora_domain::approval::ToolMatcher;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolCallResult, ToolDescription, ToolParameters, ToolPresentation};
use caelumora_domain::workspace::Workspace;
use caelumora_domain::workspace::{CommandState, CommandStatus};
use serde::Deserialize;
use serde_json::{json, Value};
use std::mem;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::time;
use tracing::warn;

/// A shell command (issue 251): its `command` as a line of code, its `output` as a terminal shows
/// it, and its exit code - what a call answers, read back as a `CommandState` (`command_state`).
pub const SHELL_PRESENTATION: ToolPresentation = ToolPresentation::new("shell");

/// Where the command `result` tells of stands, as a frontend reads it back to show it: of a call
/// presented `shell` that answered, the `CommandState` its tool gave its model; none for a call
/// presented otherwise, one that did not answer, and an answer that is no such state.
pub fn command_state(result: &ToolCallResult) -> Option<CommandState> {
    let of_a_command = result.presentation == SHELL_PRESENTATION && result.status.is_ok();
    of_a_command.then(|| serde_json::from_str(&result.content).ok()).flatten()
}

pub(super) const RUN_COMMAND: &str = "run_command";
pub(super) const COMMAND_OUTPUT: &str = "command_output";
pub(super) const KILL_COMMAND: &str = "kill_command";

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

fn wait(params: &WorkspaceParams, asked: Option<f64>) -> Duration {
    let most = params.max_wait_secs.min(MAX_WAIT_SECS) as f64;
    Duration::from_secs_f64(asked.unwrap_or(most).clamp(0.0, most))
}

/// `state` as the model is given it, by `params` and the texts `prompts`: the output cut from the
/// front to what a result may hold, and for a command still running how to go on with it
/// (`next`): a JSON object whose keys stand sorted, as `serde_json` keeps them, and which is read
/// back as the `CommandState` it holds.
fn result(params: &WorkspaceParams, prompts: &PromptTexts, mut state: CommandState) -> Value {
    if state.output.len() > params.max_output_bytes {
        let cut = state.output.ceil_char_boundary(state.output.len() - params.max_output_bytes);
        state.output.drain(..cut);
        state.truncated = true;
    }
    let running = state.status == CommandStatus::Running;
    let mut result = json!(state);
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
/// (`ToolMatcher::proposed`): for a command, those beginning with its first words, however long a
/// call waits for it.
fn followed_alike(tool: &ToolDescription, params: &Value) -> ToolMatcher {
    let mut matcher = ToolMatcher::proposed(tool, params);
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
        let guard = KillOnDrop { shell: Arc::clone(&shell), id: id.clone(), armed: true };
        let state = shell.output(&id, 0, wait(&self.params, wait_secs)).await;
        guard.disarm();
        Ok(standing(&self.params, &self.prompts, state?, workspace))
    }

    /// What the call does, as an approver reads it and its channel shows it: the command, whole.
    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        let Run { command, .. } = arguments(params)?;
        Ok(Some(format!("Run in the workspace:\n{command}")))
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

    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        arguments::<Follow>(params).map(|follow| Some(format!("Follow command {}", follow.id)))
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

    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        arguments::<Follow>(params).map(|follow| Some(format!("Kill command {}", follow.id)))
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
    use caelumora_domain::prompt_catalogue::PromptCatalogue;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::{ToolCall, ToolResultStatus};
    use caelumora_domain::workspace::testing::make_command_state;
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use std::collections::BTreeMap;

    fn the_component() -> WorkspaceComponent {
        component(json!({ "max_wait_secs": 10 }))
    }

    /// The result of a call presented `shell` that gave its model `value`, ended as `status`.
    fn ended(value: &Value, status: ToolResultStatus) -> ToolCallResult {
        let call = ToolCall { presentation: SHELL_PRESENTATION, ..make_tool_call(RUN_COMMAND) };
        ToolCallResult::of(&call, status, value.to_string(), Default::default())
    }

    /// The state of the command a call answered `value` of, read back as a frontend reads it.
    fn read_back(value: &Value) -> CommandState {
        command_state(&ended(value, ToolResultStatus::Ok)).expect("a command's state")
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
        assert!(
            ran.value.to_string().starts_with(r#"{"exit_code":0,"id":"#),
            "the model is given its keys sorted: {}",
            ran.value
        );
        let state = read_back(&ran.value);
        assert_eq!(
            (state.status, state.exit_code, state.output.as_str()),
            (CommandStatus::Completed, Some(0), "draft\nnotes.md\n")
        );
        assert_eq!(
            ran.classification.as_ref(),
            Some(&world.workspace.classification),
            "classified as the workspace"
        );
        assert!(world.dir().join("notes.md").is_file(), "written into the workspace");
    }

    /// A result is read back as a command's state only of a call presented `shell` that answered
    /// with one.
    #[test]
    fn a_result_is_a_command_s_state_only_of_a_shell_call_that_answered_one() {
        let state = make_command_state(CommandStatus::Completed);
        let written = json!(state);
        assert_eq!(command_state(&ended(&written, ToolResultStatus::Ok)), Some(state));
        let failed = ended(&written, ToolResultStatus::Failed);
        assert_eq!(command_state(&failed), None, "a call that did not answer");
        let presented_otherwise = ToolCallResult {
            presentation: ToolPresentation::DEFAULT,
            ..ended(&written, ToolResultStatus::Ok)
        };
        assert_eq!(command_state(&presented_otherwise), None, "a call presented otherwise");
        let plain = ended(&json!("done"), ToolResultStatus::Ok);
        assert_eq!(command_state(&plain), None, "an answer that is no command's state");
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
        assert!(running.value["next"].as_str().unwrap().contains(COMMAND_OUTPUT));
        assert!(running.value.get("exit_code").is_none(), "no exit code: no key");
        let begun = read_back(&running.value);
        assert_eq!((begun.status, begun.exit_code), (CommandStatus::Running, None));
        let followed = tool_named(&tool, COMMAND_OUTPUT)
            .execute(
                &json!({ "id": begun.id, "offset": begun.next_offset, "wait_secs": 0.1 }),
                None,
                &mut turn,
            )
            .await
            .unwrap();
        let followed = read_back(&followed.value);
        assert_eq!(
            (followed.status, followed.output.as_str()),
            (CommandStatus::Running, ""),
            "nothing new"
        );
        let killed = tool_named(&tool, KILL_COMMAND)
            .execute(&json!({ "id": begun.id }), None, &mut turn)
            .await
            .unwrap();
        let killed = read_back(&killed.value);
        assert_eq!(killed.status, CommandStatus::Killed);
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
                .unwrap()
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

    #[test]
    fn output_beyond_what_a_result_holds_is_cut_from_the_front() {
        let params: WorkspaceParams =
            serde_json::from_value(json!({ "max_output_bytes": 2 })).unwrap();
        let state = CommandState {
            id: "x".to_string(),
            status: CommandStatus::Completed,
            exit_code: Some(0),
            output: "abcdéf".to_string(),
            next_offset: 7,
            truncated: false,
        };
        // No text of how to go on is needed for a command that ended.
        let prompts = PromptTexts::new(Arc::new(PromptCatalogue::new()), None);
        let result = read_back(&result(&params, &prompts, state));
        assert_eq!(
            (result.output.as_str(), result.truncated),
            ("f", true),
            "the cut falls inside é and moves past it: {result:?}"
        );
    }
}
