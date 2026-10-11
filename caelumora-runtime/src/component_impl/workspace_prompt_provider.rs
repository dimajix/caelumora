//! The `workspace_prompt_provider` component (issue 251, part C7): tells the
//! agent which workspace it works on in its channel - its name, how it is
//! classified, what it holds as its owner describes it, and whether it can
//! run commands there - or that it has none here, so that it does not try.
//!
//! It says what is there, whatever tools a personality has to work on it,
//! so a personality with several gets it once. Read afresh at the start of
//! every turn through the agent's views: the workspace the channel may use
//! now (`ChannelView::workspace`). In the system prompt by default
//! (parameter `position`): it changes rarely. The part is classified as the
//! workspace, which flows into the channel where it may be used there.
//!
//! It names the files at the workspace's root that say how to work on it
//! (issue 279) - those of the parameter `instruction_files` that are there,
//! by default `AGENTS.md` and `CLAUDE.md` - and asks the agent to read them
//! with `read_file`; their content does not go into the prompt, so that
//! the part changes only when one appears or goes. That a directory may have
//! its own is said in one sentence, without looking for them. A storage
//! that cannot be reached leaves the files out, not the turn.

use crate::component::{config_of_params, ComponentError, ComponentFacets, ComponentKind, ComponentRegistry, ComponentResult};
use crate::context::{titled, Prompt, PromptPosition, PromptProvider};
use crate::terminal::{TerminalError, WorkspacePath};
use crate::view::WorkspaceView;
use crate::workspace::WorkspaceManager;
use crate::agent::work::AgentWork;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use serde::Deserialize;
use std::sync::Arc;
use tracing::warn;
use caelumora_model::security::Grant;

pub const WORKSPACE_PROMPT_PROVIDER_KIND: &str = "workspace_prompt_provider";

const TITLE: &str = "Your workspace";

#[derive(Debug)]
pub struct WorkspacePromptProvider {
    pub position: PromptPosition,
    /// The names of the files that say how to work on a workspace, each a
    /// file at its root, in the order they are named.
    instruction_files: Vec<WorkspacePath>,
    /// What looks for them (issue 296).
    workspaces: Arc<WorkspaceManager>,
}

/// What the component is configured with besides its position.
#[derive(Debug, Deserialize)]
struct WorkspacePromptConfig {
    #[serde(default = "default_instruction_files")]
    instruction_files: Vec<String>,
}

fn default_instruction_files() -> Vec<String> {
    vec!["AGENTS.md".to_string(), "CLAUDE.md".to_string()]
}

impl WorkspacePromptProvider {
    /// It with `instruction_files`, each the name of a file at a
    /// workspace's root.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` for a name that is not one of a file
    /// at the root.
    fn new(position: PromptPosition, instruction_files: &[String], workspaces: Arc<WorkspaceManager>) -> ComponentResult<Self> {
        let instruction_files = instruction_files.iter()
            .map(|name| match WorkspacePath::parse(name) {
                Ok(path) if !path.is_root() && path.parent().is_root() => Ok(path),
                _ => Err(ComponentError::InvalidConfig(format!("instruction file '{name}' is not the name of a file at the workspace's root"))),
            })
            .collect::<ComponentResult<_>>()?;
        Ok(Self { position, instruction_files, workspaces })
    }

    /// Those of the instruction files that lie at the root of `workspace`,
    /// the one `work` may use in its channel; none if its storage cannot be
    /// reached, which must not cost the turn.
    async fn instruction_files_in(&self, workspace: &WorkspaceView, work: &AgentWork<'_>) -> Vec<&WorkspacePath> {
        if self.instruction_files.is_empty() {
            return Vec::new();
        }
        let terminal = match self.workspaces.terminal_in(work.security_context(), work.session_id(), work.channel_id(), Grant::Read).await {
            Ok((_, terminal)) => terminal,
            Err(error) => {
                warn!(workspace = %workspace.id(), "Looking for the instruction files of a workspace failed: {error}");
                return Vec::new();
            }
        };
        let found = futures::future::join_all(self.instruction_files.iter().map(|file| {
            let terminal = Arc::clone(&terminal);
            async move { (file, terminal.stat(file).await) }
        })).await;
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

/// `names` as a list in a sentence, each quoted as code, the last two
/// joined by `last`: "`a`, `b` and `c`".
fn listed(names: &[&WorkspacePath], last: &str) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((only, [])) => only.clone(),
        Some((final_name, rest)) => format!("{} {last} {final_name}", rest.join(", ")),
    }
}

/// What the agent is told of `workspace`, at whose root lie `found` of the
/// instruction files `names`, and which it may change if `writable` (issue
/// 23).
fn told(workspace: &WorkspaceView, writable: bool, names: &[WorkspacePath], found: &[&WorkspacePath]) -> String {
    let mut text = format!(
        "You work on the files of the workspace \"{}\" of this session, which you share with the people and agents of \
        the channels that use it. Everything in it is classified {}.\n",
        workspace.name(),
        workspace.classification(),
    );
    if let Some(description) = workspace.description() {
        text.push_str(&format!("What it holds, as its owner describes it: {description}\n"));
    }
    text.push_str(match (writable, workspace.has_shell()) {
        (false, _) => "You may read its files, not change them, and run no commands in it.\n",
        (true, true) => "You can run shell commands in it.\n",
        (true, false) => "Its storage gives it no shell: you cannot run commands in it.\n",
    });
    if !found.is_empty() {
        let (lie, them) = if found.len() == 1 { ("lies", "it") } else { ("lie", "them") };
        text.push_str(&format!(
            "At its top {lie} {}, written for agents: how to work on its files - how they are built and tested, how they \
            are laid out, what to keep to. Read {them} with `read_file` before you work on the files.\n",
            listed(found, "and"),
        ));
    }
    if !names.is_empty() {
        let names: Vec<&WorkspacePath> = names.iter().collect();
        text.push_str(&format!(
            "A directory in it may hold a file of its own named {}, which says how to work on what lies there: read it \
            before you work there; where it disagrees with one further up, it takes precedence.\n",
            listed(&names, "or"),
        ));
    }
    titled(TITLE, &text)
}

#[async_trait]
impl PromptProvider for WorkspacePromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        // Work that reaches no session (`AgentWork::session`) has no workspace to tell of.
        let Some(session) = work.session().await else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let channel = session.channel(work.channel_id()).await?;
        Ok(match channel.workspace().await? {
            Some(workspace) => {
                let found = self.instruction_files_in(&workspace, work).await;
                let writable = workspace.may_write().await;
                Prompt::classified(told(&workspace, writable, &self.instruction_files, &found), workspace.classification().clone())
            }
            None => Prompt::classified(
                titled(TITLE, "There is no workspace you can use in this channel: no files to work on, no commands to run.\n"),
                Classification::PUBLIC,
            ),
        })
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(WORKSPACE_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>().uses::<WorkspaceManager>(),
        |descriptor, context| {
            let position = PromptPosition::of_params(&descriptor.params)?;
            let WorkspacePromptConfig { instruction_files } = config_of_params(&descriptor.params)?;
            let workspaces = Arc::clone(context.service::<WorkspaceManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(WorkspacePromptProvider::new(position, &instruction_files, workspaces)?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::testing::{session_with_workspace, WithWorkspace};
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::security::SecurityContext;

    fn provider(world: &WithWorkspace) -> WorkspacePromptProvider {
        provider_of(world, &default_instruction_files())
    }

    fn provider_of(world: &WithWorkspace, instruction_files: &[String]) -> WorkspacePromptProvider {
        WorkspacePromptProvider::new(PromptPosition::Start, instruction_files, Arc::clone(world.runtime.workspace_manager())).unwrap()
    }

    #[tokio::test]
    async fn the_agent_is_told_of_the_workspace_it_may_use_here_or_that_it_has_none() {
        let world = session_with_workspace().await;
        let provider = provider(&world);
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(prompt.text.starts_with("[Your workspace]") && prompt.text.contains("\"docs\"") && prompt.text.contains("Drafts") && prompt.text.contains("You can run shell commands"), "{}", prompt.text);
        assert_eq!(prompt.classification.as_ref(), Some(&world.workspace.classification));

        world.session.channels()[0].update_uses_workspace(&SecurityContext::system(), false).await.unwrap();
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(prompt.text.contains("There is no workspace you can use in this channel"), "{}", prompt.text);
        assert!(!prompt.text.contains("docs"), "nor which there is: {}", prompt.text);
    }

    #[tokio::test]
    async fn the_agent_is_asked_to_read_the_instruction_files_at_the_root_and_only_those_there() {
        let world = session_with_workspace().await;
        let provider = provider(&world);
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(!prompt.text.contains("At its top"), "none is there, none is named: {}", prompt.text);
        assert!(prompt.text.contains("may hold a file of its own named `AGENTS.md` or `CLAUDE.md`") && prompt.text.contains("takes precedence"), "{}", prompt.text);

        std::fs::write(world.dir().join("AGENTS.md"), "Run the tests.").unwrap();
        std::fs::create_dir(world.dir().join("CLAUDE.md")).unwrap();
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(prompt.text.contains("At its top lies `AGENTS.md`,") && prompt.text.contains("Read it with `read_file`"), "{}", prompt.text);
        assert!(!prompt.text.contains("Run the tests"), "not its content: {}", prompt.text);

        std::fs::remove_dir(world.dir().join("CLAUDE.md")).unwrap();
        std::fs::write(world.dir().join("CLAUDE.md"), "See AGENTS.md").unwrap();
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(prompt.text.contains("At its top lie `AGENTS.md` and `CLAUDE.md`,") && prompt.text.contains("Read them"), "every one found: {}", prompt.text);
    }

    #[tokio::test]
    async fn the_instruction_files_are_as_configured() {
        let world = session_with_workspace().await;
        std::fs::write(world.dir().join("AGENTS.md"), "").unwrap();
        std::fs::write(world.dir().join("GUIDE.md"), "").unwrap();
        let provider = provider_of(&world, &["GUIDE.md".to_string()]);
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(prompt.text.contains("At its top lies `GUIDE.md`,") && !prompt.text.contains("AGENTS.md"), "{}", prompt.text);

        let none = provider_of(&world, &[]);
        let mut state = AgentChannelState::default();
        let prompt = none.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(!prompt.text.contains("At its top") && !prompt.text.contains("of its own"), "{}", prompt.text);

        for invalid in ["", "docs/AGENTS.md", "../AGENTS.md", "/AGENTS.md"] {
            assert!(matches!(WorkspacePromptProvider::new(PromptPosition::Start, &[invalid.to_string()], Arc::clone(world.runtime.workspace_manager())), Err(ComponentError::InvalidConfig(_))), "{invalid:?}");
        }
    }

    #[tokio::test]
    async fn a_storage_that_cannot_be_reached_leaves_the_files_out_not_the_turn() {
        let world = session_with_workspace().await;
        std::fs::write(world.dir().join("AGENTS.md"), "").unwrap();
        std::fs::remove_dir_all(world.dir()).unwrap();
        let mut state = AgentChannelState::default();
        let prompt = provider(&world).get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(prompt.text.contains("\"docs\"") && !prompt.text.contains("At its top"), "{}", prompt.text);
    }
}
