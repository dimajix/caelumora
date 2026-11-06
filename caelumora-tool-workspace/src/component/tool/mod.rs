//! The facet `ToolProvider` of `workspace` (issues 251, 290): the tools on the session's workspace.
//!
//! - The shell (`shell`): `run_command` starts a command in the workspace's
//!   directory and follows it for a while; one still running then is handed
//!   back with its id. `command_output` follows it further, `kill_command`
//!   ends it.
//! - The files (`files`): `list_files`, `read_file`, `grep_files` and
//!   `publish_file` read; `write_file`, `replace_in_file`, `move_file` and
//!   `delete_file` change. What was read, and what was written, is given with the result as
//!   knowledge by its path (`ToolOutput::knowledge`, issue 538), which the executor records.
//!
//! A turn is offered the tools that may be called on the workspace its channel may use (issue 290):
//! none where there is none, none that change where its access list grants reading alone, no shell
//! where its storage gives none. The workspace is reached on every call all the same - to read
//! through the reader of it the situation hands out, to change through the writer the turn does
//! (issues 344, 516) -, before anything is sent, in its context (issue 296): as a participant of
//! the channel, where the channel uses it, its access list lets the agent read it - change it, for
//! the shell and the tools that write (issue 23) - and it and the channel flow both ways. A result
//! is classified as the workspace. Which calls need approval is configured as for `mcp_server`
//! (`requires_approval`, an `ApprovalMode`): all, none, or all but those that only read;
//! `command_output`, `kill_command` and `publish_file` never ask.

mod files;
mod shell;

pub(super) use files::{known, text, READ_FILE};

use super::{WorkspaceComponent, WorkspaceParams, WORKSPACE_KIND};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{
    described, proposed_matcher, workspace_reader, workspace_writer, ToolError, ToolOutput,
    ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_domain::approval::ToolMatcher;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Serialize;
use serde_json::Value;
use tracing::error;

/// Its tools - those of the shell, then those on the files -, configured by `params`, their texts
/// from `prompts`.
///
/// # Errors
/// `PromptError` if one of their texts cannot be had.
pub(super) fn tools(
    params: &WorkspaceParams,
    prompts: &PromptTexts,
) -> Result<Vec<Tool>, PromptError> {
    let mut tools = shell::tools(params, prompts)?;
    tools.extend(files::tools(params, prompts)?);
    Ok(tools)
}

/// A tool of this component, `values` told its texts (issue 57), as the
/// model is offered it.
///
/// # Errors
/// `PromptError` if one of its texts cannot be had.
fn tool(
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
    described(prompts, NAMESPACE, WORKSPACE_KIND, tool, values)
}

/// What a result says of how to go on, the text `key` of the results
/// (`results.<key>`) rendered with `values`; `None`, logged, if it cannot be
/// had - the result is given without it, as what happened is given without
/// what to do next where a message is missing (issue 57).
fn next(prompts: &PromptTexts, key: &str, values: impl Serialize) -> Option<String> {
    prompts
        .render(&format!("{NAMESPACE}.results.{key}"), values)
        .inspect_err(|e| {
            error!("A result of the workspace's tools is given without saying how to go on: {e}")
        })
        .ok()
}

/// How a call of one of its tools uses the workspace, both to be offered (issue 290) and to run:
/// reading its files, changing them, or running a command, which may change anything its place
/// reaches (issue 23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Use {
    Read,
    Change,
    Shell,
}

impl Use {
    /// How the tool `name` uses the workspace; `None` for a name that is none of its tools.
    fn of(name: &str) -> Option<Self> {
        match name {
            _ if shell::TOOLS.contains(&name) => Some(Self::Shell),
            _ if files::read_only(name) => Some(Self::Read),
            _ if files::TOOLS.contains(&name) => Some(Self::Change),
            _ => None,
        }
    }
}

#[async_trait]
impl ToolProvider for WorkspaceComponent {
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
        let workspace = workspace_reader(situation).await.map_err(|e| e.to_string())?;
        let (writable, has_shell) = (workspace.may_write().await, workspace.has_shell());
        Ok(self
            .tools()
            .into_iter()
            .filter(|tool| match Use::of(&tool.name) {
                Some(Use::Read) => true,
                Some(Use::Change) => writable,
                Some(Use::Shell) => writable && has_shell,
                None => false,
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
        let Some(using) = Use::of(name) else {
            return Err(ToolError::UnknownTool(name.to_string()));
        };
        let (output, classification) = match using {
            Use::Shell => {
                let (workspace, shell) = workspace_writer(turn).await?.shell().await?;
                let shell = shell.ok_or_else(|| {
                    ToolError::ExecutionFailed(
                        "the workspace's storage gives it no shell".to_string(),
                    )
                })?;
                (shell::execute(self, name, params, shell).await?.into(), workspace.classification)
            }
            Use::Read => {
                let (workspace, terminal) =
                    workspace_reader(&turn.situation()).await?.terminal().await?;
                let seen = (workspace.id, &workspace.classification);
                let output = files::read(self, name, params, terminal.as_ref(), seen, turn).await?;
                (output, workspace.classification)
            }
            Use::Change => {
                let (workspace, terminal) = workspace_writer(turn).await?.terminal().await?;
                let seen = (workspace.id, &workspace.classification);
                let output = files::change(name, params, terminal.as_ref(), seen, turn).await?;
                (output, workspace.classification)
            }
        };
        Ok(ToolOutput { classification: Some(classification), ..output })
    }
}

#[cfg(test)]
mod tests {
    use super::files::{
        DELETE_FILE, GREP_FILES, LIST_FILES, MOVE_FILE, PUBLISH_FILE, READ_FILE, REPLACE_IN_FILE,
        WRITE_FILE,
    };
    use super::shell::{COMMAND_OUTPUT, KILL_COMMAND, RUN_COMMAND};
    use super::*;
    use crate::component::testing::component;
    use crate::library::WorkspaceToolLibrary;
    use caelumora_agents::library::AgentsLibrary;
    use caelumora_agents::testing as agent_testing;
    use caelumora_agents::testing::workspace_turn;
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::confidentiality::{Classification, Confidentiality};
    use caelumora_domain::scope::Scope;
    use caelumora_domain::security::Grant;
    use caelumora_kernel::testing::World;
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use caelumora_security::SecurityContext;
    use serde_json::json;
    use std::fs;

    /// Issue 516: a tool that only reads the files goes through the workspace's reader - finding
    /// the workspace usable in the channel, then reading it, each asking what using it there asks
    /// to read -, and is never asked whether the agent may change them.
    #[tokio::test]
    async fn a_read_of_a_file_asks_what_reading_asks_and_never_a_change() {
        let world = World::with(&[], &[&AgentsLibrary, &WorkspaceToolLibrary], |_| {}).await;
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let alice = World::as_user(world.alice);
        let docs = world
            .kernel
            .workspace_manager()
            .create(
                &alice,
                "docs",
                Scope::UserScope { user_id: world.alice },
                "shell",
                None,
                Classification::general(Confidentiality::Internal),
            )
            .await
            .unwrap();
        let session = world.shared_session(&[helper], &[]).await;
        session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
        let place = world.files.path().join("shell").join(docs.id.to_string());
        fs::create_dir_all(place.join("notes")).unwrap();
        fs::write(place.join("notes/a.md"), "present").unwrap();
        let tool = component(json!({}));
        let mut state = AgentChannelState::default();
        let mut turn = agent_testing::turn(&session, helper, world.alice, &mut state);

        let (read, asked) = world
            .asked(tool.execute(READ_FILE, &json!({ "path": "notes/a.md" }), None, &mut turn))
            .await;
        read.expect("read");
        let using = [
            "alice as helper: GetChannel(alice's) yes",
            "alice as helper: GetWorkspace(alice's) yes",
            "alice as helper: Publish yes",
            "alice as helper: Publish yes",
        ];
        let session_read = ["alice as helper: GetSession(alice's) yes"];
        let expected: Vec<String> = [&session_read[..], &using[..], &session_read[..], &using[..]]
            .concat()
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(asked, expected, "found usable, then read, as reading is asked");
        drop(turn);
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 290: offered where the channel uses the workspace - every tool,
    /// where the agent may change its files and its storage gives a shell -
    /// and not where it does not; a call is refused there all the same.
    #[tokio::test]
    async fn no_workspace_is_offered_nor_used_where_the_channel_does_not_use_it() {
        let world = session_with_workspace().await;
        let tool = component(json!({}));
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

    #[tokio::test]
    async fn which_calls_ask_is_configured_for_all_none_or_all_but_what_only_reads() {
        let asking = |mode: Value| -> Vec<(String, bool)> {
            component(json!({ "requires_approval": mode }))
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
        let defaults = component(json!({}));
        assert!(
            defaults.tools().iter().all(|t| t.requires_approval
                || [COMMAND_OUTPUT, KILL_COMMAND, PUBLISH_FILE].contains(&t.name.as_str())),
            "all that may, unless configured"
        );
    }
}
