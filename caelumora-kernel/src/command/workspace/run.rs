//! The `/workspace run` command (issue 587): a shell command run in the session's workspace in the
//! issuer's name, and told to the channel as a call they made - `run_command`, with its result.

use crate::command::{command_error, ChannelCommand, CommandContext};
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_api::terminal::{
    kill_on_drop, CommandState, Shell, TerminalResult, MAX_OUTPUT_BYTES, RUN_COMMAND,
    SHELL_PRESENTATION,
};
use caelumora_domain::command::{CommandDescription, CommandOutput, CommandParameter};
use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolResultStatus};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::workspace::CommandStatus;
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// The name of `/workspace run`, which the terminal gives the short form `!` (issue 587).
pub const RUN_COMMAND_NAME: &str = "workspace run";

/// How long one wait for a command's output lasts before it is asked for again.
const FOLLOW_FOR: Duration = Duration::from_secs(30);

/// `/workspace run <command>`: runs a shell command in the session's workspace, until it ends, and
/// tells the channel - the agents too - as a call the issuer made, with its result.
#[derive(Debug)]
pub(super) struct RunCommand(CommandDescription);

impl RunCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(RUN_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Runs a shell command in the session's workspace until it ends, and puts \
                          it with its output into the channel, for everyone in it."
                .to_string(),
            parameters: vec![CommandParameter::required("command")],
            owners_only: false,
        })
    }
}

impl Command for RunCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for RunCommand {
    /// Runs `/workspace run` with `args`, the command: through the workspace's shell of this
    /// channel, which asks what an agent's command asks, in the issuer's name
    /// (`WorkspaceController::shell_in`) - the workspace usable here, its files theirs to change.
    /// It follows the command until it ends; dropped before, it kills the command, and nothing is
    /// told. Then it sends the call, `run_command` with the end of the command's output, as the
    /// issuer's (`UserActor::send_call`), classified as the workspace, and answers the issuer
    /// alone how the command ended.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let failed = |e: Error| command_error(RUN_COMMAND_NAME, &e);
        let session = context.session_loop(RUN_COMMAND_NAME)?;
        let controller = session.workspace().map_err(|e| failed(e.into()))?;
        let (workspace, shell) = controller
            .shell_in(
                &context.issuer.security_context(),
                &session.session(),
                &context.channel.channel(),
            )
            .await
            .map_err(|e| failed(e.into()))?;
        let shell = shell.ok_or_else(|| CommandError::Failed {
            command: RUN_COMMAND_NAME.to_string(),
            reason: "the workspace's storage runs no commands".to_string(),
        })?;
        let started_at = Utc::now();
        let id = shell.start(args).await.map_err(|e| failed(e.into()))?;
        let guard = kill_on_drop(Arc::clone(&shell), id.clone());
        let state = followed(&*shell, &id).await;
        guard.disarm();
        let state = state.map_err(|e| failed(e.into()))?;
        let ended = ended(&state);
        let call = ToolCall {
            id: Some(Uuid::new_v4().to_string()),
            function_name: RUN_COMMAND.to_string(),
            arguments: json!({ "command": args }),
            visibility: Default::default(),
            presentation: SHELL_PRESENTATION,
        };
        let content = json!(state).to_string();
        let result = ToolCallResult {
            started_at: Some(started_at),
            finished_at: Some(Utc::now()),
            ..ToolCallResult::of(&call, ToolResultStatus::Ok, content, workspace.classification)
        };
        context.issuer.send_call(call, result).await.map_err(failed)?;
        Ok(CommandOutput::private(ended))
    }
}

/// The command `id` of `shell`, followed until it ended, with the end of all it printed.
///
/// # Errors
/// A `TerminalError` of reaching it.
async fn followed(shell: &dyn Shell, id: &str) -> TerminalResult<CommandState> {
    let mut state = shell.output(id, 0, FOLLOW_FOR).await?.keeping_last(MAX_OUTPUT_BYTES);
    while state.status == CommandStatus::Running {
        let next = shell.output(id, state.next_offset, FOLLOW_FOR).await?;
        let output = state.output + &next.output;
        let truncated = state.truncated || next.truncated;
        state = CommandState { output, truncated, ..next }.keeping_last(MAX_OUTPUT_BYTES);
    }
    Ok(state)
}

/// How the command of `state` ended, in words for the issuer.
fn ended(state: &CommandState) -> String {
    match (state.status, state.exit_code) {
        (CommandStatus::Completed, Some(code)) => format!("The command exited with {code}."),
        (CommandStatus::Killed, _) => "The command was killed.".to_string(),
        (CommandStatus::Failed, _) => "The command failed.".to_string(),
        _ => "The command ended.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::SHELL_KIND;
    use crate::testing::World;
    use crate::workspace::testing::session_with_workspace;
    use crate::workspace::WorkspaceSpec;
    use caelumora_api::terminal::TerminalError;
    use caelumora_domain::actor::{ActorEvent, ActorId};
    use caelumora_domain::component::ComponentDescriptor;
    use caelumora_domain::config::AppConfig;
    use caelumora_domain::scope::Scope;
    use serde_json::Value;
    use std::sync::Mutex;
    use tokio::time;

    #[tokio::test]
    async fn a_command_runs_in_the_workspace_and_is_told_as_the_issuer_s_call() {
        let world = session_with_workspace().await;
        let owner = world.context.initiator_id.as_user_id().unwrap();
        let channel = Arc::clone(&world.session.channels()[0]);
        let mut events = channel.observe();
        let actor = channel.user_actor(owner).unwrap();

        let reply = actor
            .execute_command("workspace run echo draft > notes.md; cat notes.md")
            .await
            .expect("/workspace run runs");
        assert_eq!(reply.output.as_deref(), Some("The command exited with 0."));
        assert!(world.dir().join("notes.md").is_file(), "run in the workspace");

        let message = loop {
            let envelope = time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("the call is sent")
                .unwrap();
            if let ActorEvent::CompleteMessageGenerated(message) = envelope.event {
                break message;
            }
        };
        assert_eq!(message.sender_id, ActorId::User(owner), "the issuer's");
        assert!(message.content.is_none(), "saying nothing in words");
        let call = &message.tool_calls[0];
        assert_eq!(
            (call.function_name.as_str(), &call.arguments, call.presentation),
            (
                RUN_COMMAND,
                &json!({ "command": "echo draft > notes.md; cat notes.md" }),
                SHELL_PRESENTATION
            )
        );
        let result = &message.tool_results[0];
        assert_eq!(result.id, call.id, "the call's result");
        assert_eq!(
            result.classification, world.workspace.classification,
            "classified as the workspace"
        );
        let state: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(
            (&state["status"], &state["exit_code"], &state["output"]),
            (&json!("completed"), &json!(0), &json!("draft\n"))
        );
    }

    /// A shell whose command printed `chunks`, one each time it is asked, running until the last.
    #[derive(Debug)]
    struct Printing {
        chunks: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Shell for Printing {
        async fn start(&self, _command: &str) -> TerminalResult<String> {
            Ok("x".to_string())
        }

        async fn output(&self, id: &str, offset: u64, _: Duration) -> TerminalResult<CommandState> {
            let mut chunks = self.chunks.lock().unwrap();
            let output = chunks.remove(0);
            let ended = chunks.is_empty();
            Ok(CommandState {
                id: id.to_string(),
                status: if ended { CommandStatus::Completed } else { CommandStatus::Running },
                exit_code: ended.then_some(0),
                next_offset: offset + output.len() as u64,
                output,
                truncated: false,
            })
        }

        async fn kill(&self, id: &str, _offset: u64) -> TerminalResult<CommandState> {
            Err(TerminalError::UnknownCommand(id.to_string()))
        }
    }

    #[tokio::test]
    async fn a_command_is_followed_until_it_ended_and_the_end_of_its_output_kept() {
        let half = MAX_OUTPUT_BYTES * 2 / 3;
        let chunks = vec!["a".repeat(half), "b".repeat(half), "c".to_string()];
        let shell = Printing { chunks: Mutex::new(chunks) };
        let state = followed(&shell, "x").await.unwrap();
        assert_eq!((state.status, state.exit_code), (CommandStatus::Completed, Some(0)));
        assert_eq!(state.output.len(), MAX_OUTPUT_BYTES, "as much as a result keeps");
        assert!(state.output.ends_with(&format!("{}c", "b".repeat(half))), "its end");
        assert!(state.truncated, "and that its beginning was dropped");
    }

    /// Where the channel uses no workspace, or its storage runs no commands, the command is refused
    /// and the issuer told why.
    #[tokio::test]
    async fn a_command_is_refused_without_a_workspace_or_a_shell_saying_why() {
        let with_shell = |config: &mut AppConfig| {
            config.commands.component_nodes.push(ComponentDescriptor::new("shell", SHELL_KIND));
        };
        let world = World::configured(&[], &[], |_| {}, with_shell).await;
        let alice = World::as_user(world.alice);
        let session = world.shared_session(&[], &[]).await;
        let actor = session.channels()[0].user_actor(world.alice).unwrap();
        let reason = |error: anyhow::Error| match error.downcast::<CommandError>() {
            Ok(CommandError::Failed { reason, .. }) => reason,
            other => panic!("failed, saying why: {other:?}"),
        };

        let none = actor.execute_command("workspace run ls").await.expect_err("no workspace");
        let none = reason(none);
        assert!(none.contains("workspace"), "{none}");
        // A workspace of the storage "main", which runs no commands.
        let docs = WorkspaceSpec::new("docs", Scope::UserScope { user_id: world.alice }, "main");
        let docs = world.kernel.workspace_manager().create(&alice, docs).await.unwrap();
        session.update_workspace(&alice, Some(docs.id), None).await.unwrap();
        let shell = actor.execute_command("workspace run ls").await.expect_err("no shell");
        assert_eq!(reason(shell), "the workspace's storage runs no commands");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    #[test]
    fn the_issuer_is_told_how_the_command_ended() {
        let completed = CommandState {
            id: "x".to_string(),
            status: CommandStatus::Completed,
            exit_code: Some(2),
            output: String::new(),
            next_offset: 0,
            truncated: false,
        };
        assert_eq!(ended(&completed), "The command exited with 2.");
        let killed = CommandState { status: CommandStatus::Killed, exit_code: None, ..completed };
        assert_eq!(ended(&killed), "The command was killed.");
    }
}
