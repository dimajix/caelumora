//! The `workspace` component (issues 251, part C, and 532): the session's workspace, for the agents
//! of the channels that may use it - its files, a shell in it where its storage gives one, and a
//! prompt part that tells the agent which workspace that is. Generic over the terminal: what it can
//! do is what the workspace's storage gives, and the component knows no provider.
//!
//! Its tools:
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
//! A turn is offered the tools that may be called on the workspace its channel may use (issue 290):
//! none where there is none, none that change where its access list grants reading alone, no shell
//! where its storage gives none. The workspace is reached on every call all the same - to read
//! through the reader of it the situation hands out, to change through the writer the turn does
//! (issues 344, 516) -, before anything is sent, in its context (issue 296): as a participant of
//! the channel, where the channel uses it, its access list lets the agent read it - change it, for
//! the shell and the tools that write (issue 23) - and it and the channel flow both ways. A result
//! is classified as the workspace. Which calls need approval is configured as for `mcp_tool`
//! (`requires_approval`, an `ApprovalMode`): all, none, or all but those that only read;
//! `command_output`, `kill_command` and `publish_file` never ask.
//!
//! Its prompt part (issue 251, part C7) tells the agent which workspace it works on in its
//! channel - its name, how it is classified, what it holds as its owner describes it, and whether
//! it can run commands there - or that it has none here, so that it does not try. Read afresh at
//! the start of every turn through the kernel's reader of it: the workspace the channel may use now
//! (`WorkspaceReader`, issue 516). In the system prompt by default (parameter `position`): it
//! changes rarely. The part is classified as the workspace, which flows into the channel where it
//! may be used there.
//!
//! It names the files at the workspace's root that say how to work on it (issue 279) - those of the
//! parameter `instruction_files` that are there, by default `AGENTS.md` and `CLAUDE.md` - and asks
//! the agent to read them with `read_file`; their content does not go into the prompt, so that the
//! part changes only when one appears or goes. That a directory may have its own is said in one
//! sentence, without looking for them. A storage that cannot be reached leaves the files out, not
//! the turn.

pub use files::{
    DELETE_FILE, GREP_FILES, LIST_FILES, MOVE_FILE, PUBLISH_FILE, READ_FILE, REPLACE_IN_FILE,
    WRITE_FILE,
};
pub use shell::{COMMAND_OUTPUT, KILL_COMMAND, RUN_COMMAND};

use crate::{files, shell, NAMESPACE};
use async_trait::async_trait;
use caelumora_agents::prompt::{
    texts_of, writing_for_the_model, ContextError, Prompt, PromptPosition, PromptProvider,
};
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{
    described, proposed_matcher, tool_text, workspace_reader, workspace_writer, ApprovalMode,
    ToolError, ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_api::component::{config_of_params, ComponentError, ComponentResult, Params};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_api::terminal::{TerminalError, WorkspacePath};
use caelumora_api::workspace::{WorkspaceReader, WorkspaceWriter};
use caelumora_domain::approval::ToolMatcher;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use futures::future;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tracing::{error, warn};

pub const WORKSPACE_KIND: &str = "workspace";

fn default_wait_secs() -> u64 {
    60
}

fn default_output_bytes() -> usize {
    64 * 1024
}

fn default_instruction_files() -> Vec<String> {
    vec!["AGENTS.md".to_string(), "CLAUDE.md".to_string()]
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceConfig {
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
    /// Where its prompt part goes; the system prompt unless configured.
    #[serde(default)]
    position: PromptPosition,
    /// The names of the files that say how to work on a workspace, each a file at its root:
    /// `AGENTS.md` and `CLAUDE.md` unless configured.
    #[serde(default = "default_instruction_files")]
    instruction_files: Vec<String>,
}

impl WorkspaceConfig {
    /// Its `instruction_files`, each as a file at a workspace's root.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` for a name that is not one of a file at the root.
    fn instruction_files(&self) -> ComponentResult<Vec<WorkspacePath>> {
        self.instruction_files
            .iter()
            .map(|name| match WorkspacePath::parse(name) {
                Ok(path) if !path.is_root() && path.parent().is_root() => Ok(path),
                _ => Err(ComponentError::InvalidConfig(format!(
                    "instruction file '{name}' is not the name of a file at the workspace's root"
                ))),
            })
            .collect()
    }
}

impl Params for WorkspaceConfig {
    /// # Errors
    /// `InvalidConfig` for an instruction file that is not one at a workspace's root.
    fn check(&self) -> ComponentResult<()> {
        self.instruction_files().map(drop)
    }
}

#[derive(Debug)]
pub struct WorkspaceComponent {
    pub(crate) config: WorkspaceConfig,
    /// What its tool set is for and its tools, as the configuration or the
    /// prompt catalogue words them (issue 57).
    description: String,
    tools: Vec<Tool>,
    /// The files that say how to work on a workspace, each a file at its root, in the order they
    /// are named.
    instruction_files: Vec<WorkspacePath>,
    /// What its prompt part says, and what its results say of how to go on.
    pub(crate) prompts: PromptTexts,
}

impl WorkspaceComponent {
    /// The component, configured by `config`, its texts from `prompts`.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` for an instruction file that is not one at a workspace's
    /// root, `ComponentError::Prompt` if one of its texts cannot be had.
    pub(crate) fn new(config: WorkspaceConfig, prompts: PromptTexts) -> ComponentResult<Self> {
        let mut tools = shell::tools(&config, &prompts)?;
        tools.extend(files::tools(&config, &prompts)?);
        let description = match &config.description {
            Some(description) => description.clone(),
            None => tool_text(&prompts, NAMESPACE, WORKSPACE_KIND, "description", ())?,
        };
        let instruction_files = config.instruction_files()?;
        Ok(Self { config, description, tools, instruction_files, prompts })
    }

    /// Those of the instruction files that lie at the root of `workspace`, the one the work may use
    /// in its channel; none if its storage cannot be reached, which must not cost the turn.
    async fn instruction_files_in(&self, workspace: &dyn WorkspaceReader) -> Vec<&WorkspacePath> {
        if self.instruction_files.is_empty() {
            return Vec::new();
        }
        let terminal = match workspace.terminal().await {
            Ok((_, terminal)) => terminal,
            Err(error) => {
                warn!(workspace = %workspace.id(), "Looking for the instruction files of a workspace failed: {error}");
                return Vec::new();
            }
        };
        let found = future::join_all(self.instruction_files.iter().map(|file| {
            let terminal = Arc::clone(&terminal);
            async move { (file, terminal.stat(file).await) }
        }))
        .await;
        found.into_iter()
            .filter_map(|(file, entry)| match entry {
                Ok(entry) => (!entry.is_dir).then_some(file),
                Err(TerminalError::NotFound(_)) => None,
                Err(error) => {
                    warn!(workspace = %workspace.id(), "Looking for an instruction file of a workspace failed: {error}");
                    None
                }
            })
            .collect()
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
    described(prompts, NAMESPACE, WORKSPACE_KIND, tool, values)
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
        let (value, classification) = match using {
            Use::Shell => {
                let (workspace, shell) = workspace_writer(turn).await?.shell().await?;
                let shell = shell.ok_or_else(|| {
                    ToolError::ExecutionFailed(
                        "the workspace's storage gives it no shell".to_string(),
                    )
                })?;
                (shell::execute(self, name, params, shell).await?, workspace.classification)
            }
            Use::Read => {
                let (workspace, terminal) =
                    workspace_reader(&turn.situation()).await?.terminal().await?;
                let seen = (workspace.id, &workspace.classification);
                let value = files::read(self, name, params, terminal.as_ref(), seen, turn).await?;
                (value, workspace.classification)
            }
            Use::Change => {
                let (workspace, terminal) = workspace_writer(turn).await?.terminal().await?;
                let seen = (workspace.id, &workspace.classification);
                let value = files::change(name, params, terminal.as_ref(), seen, turn).await?;
                (value, workspace.classification)
            }
        };
        Ok(ToolOutput::classified(value, classification))
    }
}

/// What the agent is told of `workspace`, at whose root lie `found` of the
/// instruction files `names`, and which it may change if `writable` (issue
/// 23), worded by `prompts`.
fn told(
    prompts: &PromptTexts,
    workspace: &dyn WorkspaceReader,
    writable: bool,
    names: &[WorkspacePath],
    found: &[&WorkspacePath],
) -> Result<String, PromptError> {
    let listed = |paths: &mut dyn Iterator<Item = &WorkspacePath>| {
        paths.map(|path| path.to_string()).collect::<Vec<_>>()
    };
    prompts.render(
        &format!("{NAMESPACE}.workspace"),
        context! {
            name => workspace.name(),
            classification => workspace.classification().to_string(),
            description => workspace.description().map(|description| description.to_string()),
            writable,
            shell => workspace.has_shell(),
            found => listed(&mut found.iter().copied()),
            names => listed(&mut names.iter()),
            read_file => READ_FILE,
        },
    )
}

#[async_trait]
impl PromptProvider for WorkspaceComponent {
    fn position(&self) -> PromptPosition {
        self.config.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        Ok(match situation.reader::<dyn WorkspaceReader>().await {
            Some(workspace) => {
                let found = self.instruction_files_in(workspace.as_ref()).await;
                let writable = workspace.may_write().await;
                Prompt::classified(
                    told(
                        &self.prompts,
                        workspace.as_ref(),
                        writable,
                        &self.instruction_files,
                        &found,
                    )?,
                    workspace.classification().clone(),
                )
            }
            None => Prompt::classified(
                self.prompts.render(&format!("{NAMESPACE}.no_workspace"), ())?,
                Classification::PUBLIC,
            ),
        })
    }
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(WORKSPACE_KIND)
                .params::<WorkspaceConfig>()
                .provides::<dyn ToolProvider>()
                .provides::<dyn PromptProvider>()
                .reads::<dyn WorkspaceReader>()
                .writes::<dyn WorkspaceWriter>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let config: WorkspaceConfig = config_of_params(&descriptor.params)?;
            let component = Arc::new(WorkspaceComponent::new(config, prompts)?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::<WorkspaceComponent>::clone(&component));
            facets.insert::<dyn PromptProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::files::READ_FILE;
    use crate::library::WorkspaceToolLibrary;
    use crate::{catalogue, prompts};
    use caelumora_agents::library::AgentsLibrary;
    use caelumora_agents::testing as agent_testing;
    use caelumora_agents::testing::with_agent_kinds;
    use caelumora_agents::testing::workspace_turn;
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::confidentiality::Confidentiality;
    use caelumora_domain::scope::Scope;
    use caelumora_domain::security::Grant;
    use caelumora_domain::type_map::TypeKey;
    use caelumora_kernel::component::BuildContext;
    use caelumora_kernel::component::ComponentRegistry;
    use caelumora_kernel::testing::World;
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use caelumora_security::{Policies, SecurityContext};
    use caelumora_store::{EntityStores, Persistence};
    use serde_json::json;
    use std::fs;

    /// The component with `params`.
    pub(crate) fn component(params: Value) -> WorkspaceComponent {
        WorkspaceComponent::new(serde_json::from_value(params).unwrap(), prompts()).unwrap()
    }

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

    /// Written by hand without `params`, as the seed does: every parameter
    /// takes its default, and it provides its tools and its prompt part.
    #[tokio::test]
    async fn is_built_without_parameters() {
        let mut registry = ComponentRegistry::new(
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
        );
        // The agents' kinds, with a model whose profile names the variant.
        with_agent_kinds(&mut registry);
        register(&mut registry.library_components());
        let config = ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("workspace", WORKSPACE_KIND)],
        };
        let world = session_with_workspace().await;
        // The world runs the kernel alone: the texts of these tools are added.
        let context = BuildContext::new(
            SecurityContext::default(),
            world.kernel.services().clone().with(catalogue()),
        );
        let facets = [TypeKey::of::<dyn ToolProvider>(), TypeKey::of::<dyn PromptProvider>()];
        let container = (Arc::new(registry)).build(&config, &facets, &context).await.unwrap();
        assert_eq!(container.get_all::<dyn ToolProvider>(None).unwrap().len(), 1, "its tools");
        assert_eq!(container.get_all::<dyn PromptProvider>(None).unwrap().len(), 1, "its part");
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

    #[tokio::test]
    async fn the_agent_is_told_of_the_workspace_it_may_use_here_or_that_it_has_none() {
        let world = session_with_workspace().await;
        let provider = component(json!({}));
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.starts_with("[Your workspace]")
                && prompt.text.contains("\"docs\"")
                && prompt.text.contains("Drafts")
                && prompt.text.contains("You can run shell commands"),
            "{}",
            prompt.text
        );
        assert_eq!(prompt.classification.as_ref(), Some(&world.workspace.classification));

        world.session.channels()[0]
            .update_uses_workspace(&SecurityContext::system(), false)
            .await
            .unwrap();
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.contains("There is no workspace you can use in this channel"),
            "{}",
            prompt.text
        );
        assert!(!prompt.text.contains("docs"), "nor which there is: {}", prompt.text);
    }

    #[tokio::test]
    async fn the_agent_is_asked_to_read_the_instruction_files_at_the_root_and_only_those_there() {
        let world = session_with_workspace().await;
        let provider = component(json!({}));
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            !prompt.text.contains("At its top"),
            "none is there, none is named: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("may hold a file of its own named `AGENTS.md` or `CLAUDE.md`")
                && prompt.text.contains("takes precedence"),
            "{}",
            prompt.text
        );

        fs::write(world.dir().join("AGENTS.md"), "Run the tests.").unwrap();
        fs::create_dir(world.dir().join("CLAUDE.md")).unwrap();
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.contains("At its top lies `AGENTS.md`,")
                && prompt.text.contains("Read it with `read_file`"),
            "{}",
            prompt.text
        );
        assert!(!prompt.text.contains("Run the tests"), "not its content: {}", prompt.text);

        fs::remove_dir(world.dir().join("CLAUDE.md")).unwrap();
        fs::write(world.dir().join("CLAUDE.md"), "See AGENTS.md").unwrap();
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.contains("At its top lie `AGENTS.md` and `CLAUDE.md`,")
                && prompt.text.contains("Read them"),
            "every one found: {}",
            prompt.text
        );
    }

    #[tokio::test]
    async fn the_instruction_files_are_as_configured() {
        let world = session_with_workspace().await;
        fs::write(world.dir().join("AGENTS.md"), "").unwrap();
        fs::write(world.dir().join("GUIDE.md"), "").unwrap();
        let provider = component(json!({ "instruction_files": ["GUIDE.md"] }));
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.contains("At its top lies `GUIDE.md`,")
                && !prompt.text.contains("AGENTS.md"),
            "{}",
            prompt.text
        );

        let none = component(json!({ "instruction_files": [] }));
        let mut state = AgentChannelState::default();
        let prompt =
            none.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            !prompt.text.contains("At its top") && !prompt.text.contains("of its own"),
            "{}",
            prompt.text
        );

        for invalid in ["", "docs/AGENTS.md", "../AGENTS.md", "/AGENTS.md"] {
            let params = json!({ "instruction_files": [invalid] });
            assert!(
                matches!(
                    config_of_params::<WorkspaceConfig>(&params),
                    Err(ComponentError::InvalidConfig(_))
                ),
                "{invalid:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_storage_that_cannot_be_reached_leaves_the_files_out_not_the_turn() {
        let world = session_with_workspace().await;
        fs::write(world.dir().join("AGENTS.md"), "").unwrap();
        fs::remove_dir_all(world.dir()).unwrap();
        let mut state = AgentChannelState::default();
        let prompt = component(json!({}))
            .get_prompt(&workspace_turn(&world, &mut state).situation())
            .await
            .unwrap();
        assert!(
            prompt.text.contains("\"docs\"") && !prompt.text.contains("At its top"),
            "{}",
            prompt.text
        );
    }
}
